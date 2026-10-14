// 把 three.js（+ OrbitControls）打成一个自包含的 ES 模块，供「演示页」离线使用。
//
// 为什么要这么绕：three 从 r160 起把构建拆成 `three.module.js` + `three.core.js`
// （两者加起来 2 MB 未压缩），而且 npm 包里**不再附 min 版本**；OrbitControls
// 又是 `from 'three'` 的裸模块名，浏览器直接加载要么找不到、要么得自己写 import map。
// 于是这里在构建机上一次性地打成**单文件 + min**（约 750 KB），提交进
// src-tauri/assets/vendor/，由 `lhpage://` 协议按 `/_vendor/three.js` 提供给页面。
//
// 用法（要升版本时跑一遍）：
//   node scripts/vendor-three.mjs [版本，默认看下面 THREE_VERSION]
// 产出：
//   src-tauri/assets/vendor/three.bundle.min.js   ← 页面里 import 的就是它
//   src-tauri/assets/vendor/three.LICENSE.txt     ← MIT 许可原文，一起提交
//
// 只有这一个第三方库是有意为之：三维演示几乎绕不开它，而「演示页依赖 CDN」在
// 断网/被墙时就是一片空白。其它库（d3 / chart.js…）不预置——要联网才能用时提示词
// 会说明，别把资产目录堆成 CDN。

import { execFileSync } from "node:child_process";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { join, resolve } from "node:path";

const THREE_VERSION = process.argv[2] ?? "0.186.1";
const root = resolve(import.meta.dirname, "..");
const outDir = join(root, "src-tauri", "assets", "vendor");
const stage = join(root, "src-tauri", "target", "vendor-three");

// esbuild 的平台二进制：走 vite 已经装好的那份，不额外拉依赖
const esbuild = [
  join(root, "node_modules", "@esbuild", "win32-x64", "esbuild.exe"),
  join(root, "node_modules", "@esbuild", "darwin-arm64", "bin", "esbuild"),
  join(root, "node_modules", "@esbuild", "linux-x64", "bin", "esbuild"),
].find((p) => existsSync(p));
if (!esbuild) {
  console.error("找不到 esbuild 的平台二进制（先 npm install，vite 会把它装上）。");
  process.exit(1);
}

rmSync(stage, { recursive: true, force: true });
mkdirSync(stage, { recursive: true });

console.log(`下载 three@${THREE_VERSION} …`);
execFileSync("npm", ["pack", `three@${THREE_VERSION}`], { cwd: stage, stdio: "inherit", shell: true });
const tgz = join(stage, `three-${THREE_VERSION}.tgz`);
if (!existsSync(tgz)) {
  console.error(`没拿到 ${tgz}，npm pack 的输出不对？`);
  process.exit(1);
}
execFileSync("tar", ["-xzf", tgz, "-C", stage], { stdio: "inherit" });

// 入口：把 three 的全部导出 + OrbitControls 一起再导出，打成一个模块。
// 页面里就一句 `import { Scene, OrbitControls } from "/_vendor/three.js"`。
const entry = join(stage, "entry.js");
writeFileSync(
  entry,
  `export * from ${JSON.stringify(join(stage, "package", "build", "three.module.js"))};\n` +
    `export { OrbitControls } from ${JSON.stringify(join(stage, "package", "examples", "jsm", "controls", "OrbitControls.js"))};\n`,
);

const out = join(outDir, "three.bundle.min.js");
mkdirSync(outDir, { recursive: true });
console.log("打包 + 压缩 …");
execFileSync(
  esbuild,
  [entry, "--bundle", "--format=esm", "--minify", "--legal-comments=none", `--outfile=${out}`],
  { stdio: "inherit" },
);

copyFileSync(join(stage, "package", "LICENSE"), join(outDir, "three.LICENSE.txt"));

const size = statSync(out).size;
const head = readFileSync(out, "utf8").slice(0, 80);
console.log(`产出：${out}`);
console.log(`大小：${(size / 1024).toFixed(0)} KB`);
console.log(`开头：${head}`);
console.log(
  "\n记得把版本号写进 src-tauri/assets/vendor/README.md，并重跑 npm run rust:check" +
    "（它是 include_bytes! 编进二进制的，改了必须重新编译）。",
);
