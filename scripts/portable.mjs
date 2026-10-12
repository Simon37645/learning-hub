// 打包「绿色版」：解压即用的 zip（和安装包一起发到 release 里）。
//
// 为什么单独做一份：安装版会写注册表、装到 %LOCALAPPDATA%，而有人就想
// 拷到 U 盘里带着走、或者不想在别人机器上留痕迹。绿色版只有一个 exe + 图标
// + 一份说明，删掉文件夹就算卸载干净。
//
// 用法：
//   node scripts/portable.mjs            先 tauri build 过之后跑
//   npm run portable                     等价（见 package.json）
//
// 说明：exe 与安装版是同一个二进制（Tauri 的 NSIS 安装器只是把它装到别处），
// 所以绿色版不需要额外编译，直接复制出来即可。WebView2 运行时是系统组件，
// Win11 与打过更新的 Win10 都自带——这一点写在说明文件里，免得用户以为缺文件。

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const version = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;

const exe = join(root, "src-tauri", "target", "release", "learning-hub.exe");
if (!existsSync(exe)) {
  console.error(`找不到可执行文件：${exe}\n先跑 npm run app:build（或 npm run dist）。`);
  process.exit(1);
}
const icon = join(root, "src-tauri", "icons", "icon.ico");
if (!existsSync(icon)) {
  console.error(`找不到图标：${icon}\n先跑 npm run icon。`);
  process.exit(1);
}

// 组装一个临时目录，再整份压成 zip（路径保持平铺，解压出来就是三个文件）
const stage = join(root, "src-tauri", "target", "release", "portable");
rmSync(stage, { recursive: true, force: true });
mkdirSync(stage, { recursive: true });
copyFileSync(exe, join(stage, "learning-hub.exe"));
copyFileSync(icon, join(stage, "app.ico"));

const readme = `学习中枢 v${version} · 绿色版（portable）
${"=".repeat(46)}

双击 learning-hub.exe 就能用，不需要安装；整个文件夹删掉即卸载干净。

- 系统要求：Windows 10/11 x64，需要 WebView2 运行时（Win11 与打过更新的 Win10 自带）。
- 配置与模型档案存在 %APPDATA%\\com.learninghub.desktop\\config.json，和安装版共用同一份。
- 工作区默认在「文档\\学习中枢」，可在「设置 → 工作区」里改。
- 想让桌面快捷方式指到这里：在源码目录里跑 npm run shortcut。
- 手机远程控制：侧栏「远程控制」→ 打开后扫码即可（同一 WiFi 直接连；
  在外面用就填「中继服务器」，见 docs/REMOTE.md）。
`;
writeFileSync(join(stage, "使用说明.txt"), readme, "utf8");

const outDir = join(root, "release");
mkdirSync(outDir, { recursive: true });
const target = join(outDir, `LearningHub-${version}-x64-portable.zip`);
rmSync(target, { force: true });

// 用 PowerShell 的 Compress-Archive：Windows 自带，不引第三方打包库。
// 传的是文件列表（不是目录），所以 zip 里是平铺的三个文件。
const files = ["learning-hub.exe", "app.ico", "使用说明.txt"].map((f) => `'${join(stage, f)}'`).join(",");
execFileSync(
  "powershell",
  ["-NoProfile", "-NonInteractive", "-Command", `Compress-Archive -Path ${files} -DestinationPath '${target}' -Force`],
  { stdio: "inherit" },
);

const bytes = statSync(target).size;
const sha = createHash("sha256").update(readFileSync(target)).digest("hex").toUpperCase();
console.log(`绿色版：${target}`);
console.log(`大小：${(bytes / 1024 / 1024).toFixed(2)} MiB`);
console.log(`SHA256：${sha}`);
