// 在桌面创建「学习中枢」快捷方式，指向打包版。
//
// 为什么要绕这一道：PowerShell 5.1 读取无 BOM 的 .ps1 会按 ANSI 解码，
// 脚本里的中文会变成乱码（快捷方式名字就花了）。这里把整段脚本编成
// UTF-16LE 的 base64 交给 -EncodedCommand，彻底绕开编码问题。
//
// 用法：node scripts/make-shortcut.mjs

import { execFileSync } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const exe = join(root, "release", "app", "learning-hub.exe");
const icon = join(root, "release", "app", "app.ico");

if (!existsSync(exe)) {
  console.error(`找不到打包版：${exe}\n先跑 npm run dist 生成。`);
  process.exit(1);
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
