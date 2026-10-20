// 找出「移植进来的 InkNote 样式表」和应用自身组件**撞类名**的地方。
//
// 为什么需要它：src/inknote/editor.css 是全局引入的（main.tsx），它里面那些
// 通用类名（.toast / .hint …）会直接盖到应用自己的同名样式上，而且 CSS 只看
// 优先级和顺序，不看「谁写的」——症状常常是「某个角落的提示跑到窗口外面去了」
// 这种一眼看不出原因的现象（真踩过：右下角 toast 被改成 fixed + left:50%，
// 而且 bottom 用了未定义变量，提示有一半在窗口外）。
//
// 用法：node scripts/style-collisions.mjs
// 有新冲突时按提示给 inknote 的规则加 `.inknote-scope` 前缀（或给应用换个类名）。

import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");

/** 收集 tsx/ts 里 className 用到的类名 */
function classesIn(dir, out = new Set()) {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) {
      classesIn(p, out);
      continue;
    }
    if (!/\.(tsx|ts)$/.test(name)) continue;
    const src = readFileSync(p, "utf8");
    for (const m of src.matchAll(/className=(?:"([^"]*)"|\{`([^`]*)`\}|\{"([^"]*)")/g)) {
      const raw = (m[1] ?? m[2] ?? m[3] ?? "").replace(/\$\{[^}]*\}/g, " ");
      for (const c of raw.split(/\s+/)) if (c && !c.includes("{") && !c.includes("$")) out.add(c);
    }
  }
  return out;
}

/** 收集样式表里的类选择器，并记下它是否已被 .inknote-scope 限定 */
function cssClasses(file) {
  const src = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const out = new Map();
  for (const m of src.matchAll(/(^|[},])\s*([^{}]+)\{/g)) {
    const selector = m[2].trim();
    if (selector.startsWith("@")) continue;
    for (const part of selector.split(",")) {
      const scoped = part.includes(".inknote-scope");
      for (const c of part.matchAll(/\.([a-zA-Z][\w-]*)/g)) {
        out.set(c[1], (out.get(c[1]) ?? false) || scoped);
      }
    }
  }
  return out;
}

const appClasses = classesIn(join(root, "src", "components"));
const ink = cssClasses(join(root, "src", "inknote", "editor.css"));

const bad = [];
for (const [cls, scoped] of ink) {
  if (scoped) continue;
  if (appClasses.has(cls)) bad.push(cls);
}
bad.sort();

// ---------------------------------------------------------------- KaTeX
//
// 第三方样式表（katex.min.css 在 main.tsx 里全局引入）给公式内部元素挂的类名
// （.overlay / .tag / .accent …）同样会被 app.css 里**单类名的全局规则**劫持。
// 真踩过：`.overlay`（弹窗遮罩）把 KaTeX `\vec` 的定位层改成 position: fixed +
// 深色底，公式一渲染就在视口左上角钉出一个盖住标题栏的黑方块。
//
// 只认「单类名选择器」（`.foo` / `.foo:not(…)`）：多类名的规则（`.tag.accent`）
// 要求元素同时带好几个类，KaTeX 的元素不会命中。带 `:not(.katex …)` 的规则
// 已经把自己挡在 KaTeX 之外，跳过。
const katex = cssClasses(join(root, "node_modules", "katex", "dist", "katex.min.css"));
/** 故意给 KaTeX 自己的容器加的样式（显示公式的排版），不算冲突 */
const KATEX_ALLOWED = new Set(["katex-display"]);

function singleClassRules(file) {
  const src = readFileSync(file, "utf8").replace(/\/\*[\s\S]*?\*\//g, "");
  const out = new Set();
  for (const m of src.matchAll(/(^|[},])\s*([^{}]+)\{/g)) {
    const selector = m[2].trim();
    if (selector.startsWith("@")) continue;
    for (const part of selector.split(",")) {
      const p = part.trim();
      if (p.includes(".inknote-scope")) continue;
      if (/:not\([^)]*\.katex[^)]*\)/.test(p)) continue;
      const cleaned = p.replace(/:(not|is|where)\([^)]*\)/g, "").trim();
      const one = cleaned.match(/^\.([a-zA-Z][\w-]*)$/);
      if (one) out.add(one[1]);
    }
  }
  return out;
}

const katexBad = [...singleClassRules(join(root, "src", "styles", "app.css"))]
  .filter((cls) => katex.has(cls) && !KATEX_ALLOWED.has(cls))
  .sort();

if (bad.length === 0 && katexBad.length === 0) {
  console.log("✓ 没有类名冲突（inknote 的通用类名都已限定在 .inknote-scope 内；app.css 没有单类名规则撞 KaTeX）");
  process.exit(0);
}
if (bad.length > 0) {
  console.error(`✗ 有 ${bad.length} 个类名冲突：${bad.join(", ")}`);
  console.error("  给 src/inknote/editor.css 里对应的规则加 `.inknote-scope` 前缀，或给应用换个类名。");
}
if (katexBad.length > 0) {
  console.error(`✗ app.css 有 ${katexBad.length} 个单类名规则撞 KaTeX：${katexBad.join(", ")}`);
  console.error("  给应用换个类名，或用 `:not(.katex *)` 把它挡在 KaTeX 之外（见 .tag 的写法）。");
}
process.exit(1);
