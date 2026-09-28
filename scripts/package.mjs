// 打包收尾：把 Tauri 的 NSIS 产物整理成一个可以直接发布的安装包。
//
// 为什么需要这一步：productName 是中文（学习中枢），所以 NSIS 产物名也是中文
// （学习中枢_0.1.0_x64-setup.exe）。而 GitHub 的 release 附件名会剥掉非 ASCII 字符，
// 上传后就变成 "_0.1.0_x64-setup.exe" —— 看起来像坏了。
// 所以这里复制一份成 ASCII 名（LearningHub-<版本>-x64-setup.exe），
// 并打印大小与 SHA256，方便贴进 release notes。
//
// 用法：
//   npm run dist                     先 tauri build，再整理产物
//   node scripts/package.mjs         只整理（已经 build 过）

import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const version = JSON.parse(readFileSync(join(root, "package.json"), "utf8")).version;

const bundleDir = join(root, "src-tauri", "target", "release", "bundle", "nsis");
if (!existsSync(bundleDir)) {
  console.error(`找不到打包产物目录：${bundleDir}\n先跑 npm run app:build 或 npm run dist。`);
  process.exit(1);
}

const installer = readdirSync(bundleDir).find((f) => f.endsWith("-setup.exe"));
if (!installer) {
  console.error(`打包产物目录里没有 *-setup.exe：${bundleDir}`);
  process.exit(1);
}

const target = `LearningHub-${version}-x64-setup.exe`;
const outDir = join(root, "release");
mkdirSync(outDir, { recursive: true });
const out = join(outDir, target);
copyFileSync(join(bundleDir, installer), out);

const bytes = statSync(out).size;
const sha = createHash("sha256").update(readFileSync(out)).digest("hex").toUpperCase();

console.log(`安装包：${out}`);
console.log(`原始产物：${installer}`);
console.log(`大小：${(bytes / 1024 / 1024).toFixed(2)} MiB`);
console.log(`SHA256：${sha}`);
console.log("");
console.log("发布用：");
console.log(`  gh release create v${version} "${out}" --title "v${version}" --notes-file <说明文件>`);
