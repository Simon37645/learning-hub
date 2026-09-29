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

if (bad.length === 0) {
  console.log("✓ 没有类名冲突（inknote 的通用类名都已限定在 .inknote-scope 内）");
  process.exit(0);
}
console.error(`✗ 有 ${bad.length} 个类名冲突：${bad.join(", ")}`);
console.error("  给 src/inknote/editor.css 里对应的规则加 `.inknote-scope` 前缀，或给应用换个类名。");
process.exit(1);
