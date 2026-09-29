// 在桌面创建「学习中枢」快捷方式，指向打包版。
//
// 为什么要绕这一道：PowerShell 5.1 读取无 BOM 的 .ps1 会按 ANSI 解码，
// 脚本里的中文会变成乱码（快捷方式名字就花了）。这里把整段脚本编成
// UTF-16LE 的 base64 交给 -EncodedCommand，彻底绕开编码问题。
//
// 快捷方式指向 release/app/ 下的绿色副本，而不是 src-tauri/target/release/
// ——后者会被 cargo clean 清掉，快捷方式就成死链了。副本由本脚本从构建产物
// 同步过来，所以「npm run dist && npm run shortcut」就是「重装到桌面」。
//
// 用法：node scripts/make-shortcut.mjs

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, statSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const appDir = join(root, "release", "app");
const exe = join(appDir, "learning-hub.exe");
const icon = join(appDir, "app.ico");

// 从构建产物同步绿色副本。应用正在运行时 Windows 会锁住 exe，
// 这时不要去动它：报一句人话，继续把快捷方式维护好。
const built = join(root, "src-tauri", "target", "release", "learning-hub.exe");
const builtIcon = join(root, "src-tauri", "icons", "icon.ico");
mkdirSync(appDir, { recursive: true });

for (const [from, to, label] of [
  [built, exe, "可执行文件"],
  [builtIcon, icon, "图标"],
]) {
  if (!existsSync(from)) {
    if (existsSync(to)) continue; // 没重新构建，用现有副本
    console.error(`找不到${label}：${from}\n先跑 npm run app:build 或 npm run dist。`);
    process.exit(1);
  }
  const same =
    existsSync(to) &&
    statSync(from).size === statSync(to).size &&
    readFileSync(from).equals(readFileSync(to));
  if (same) {
    console.log(`${label}已是最新：${to}`);
    continue;
  }
  try {
    copyFileSync(from, to);
    console.log(`${label}已更新：${to}（${(statSync(to).size / 1024 / 1024).toFixed(1)} MiB）`);
  } catch (err) {
    if (existsSync(to)) {
      console.error(
        `更新${label}失败（应用可能正在运行，先关掉它）：${err.code ?? err.message}\n` +
          `快捷方式仍指向现有副本：${to}`,
      );
    } else {
      throw err;
    }
  }
}

const desktop = join(homedir(), "Desktop");
const lnk = join(desktop, "学习中枢.lnk");

const script = `
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$sh = New-Object -ComObject WScript.Shell
$lnk = $sh.CreateShortcut(${JSON.stringify(lnk)})
$lnk.TargetPath = ${JSON.stringify(exe)}
$lnk.WorkingDirectory = ${JSON.stringify(join(root, "release", "app"))}
$lnk.IconLocation = ${JSON.stringify(icon)} + ',0'
$lnk.Description = '学习中枢 — 个人 AI 学习助手'
$lnk.WindowStyle = 1
$lnk.Save()
Write-Output ('created: ' + $lnk.FullName)
`;

const encoded = Buffer.from(script, "utf16le").toString("base64");
const out = execFileSync(
  "powershell",
  ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", encoded],
  { encoding: "utf8" },
);
process.stdout.write(out);

// 回读确认（同样用 EncodedCommand，避免中文路径被转码）
const verify = `
$ProgressPreference = 'SilentlyContinue'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$sh = New-Object -ComObject WScript.Shell
$l = $sh.CreateShortcut(${JSON.stringify(lnk)})
[pscustomobject]@{
  Name = (Get-Item ${JSON.stringify(lnk)}).Name
  Target = $l.TargetPath
  Icon = $l.IconLocation
  TargetExists = (Test-Path $l.TargetPath)
} | ConvertTo-Json -Compress
`;
const vEncoded = Buffer.from(verify, "utf16le").toString("base64");
const vOut = execFileSync(
  "powershell",
  ["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", vEncoded],
  { encoding: "utf8" },
);
console.log("verify:", vOut.trim());

// 顺带确认这个 exe 确实是打包版（前端已内嵌，不依赖 dev server）
const size = readFileSync(exe).length;
console.log(`打包版可执行文件：${exe}（${(size / 1024 / 1024).toFixed(1)} MiB）`);
