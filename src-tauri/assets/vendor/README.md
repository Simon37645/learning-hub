# 随应用内置的前端资产（演示页用）

这里的东西会被 `include_bytes!` 编进二进制，由 `lhpage://` 协议按 `/_vendor/<名字>` 提供给
agent 建的演示页（见 `src-tauri/src/viewer/pages.rs`）。为什么要有这么一份：演示页要能离线打开，
而「三维演示」几乎绕不开 three.js。

| 文件 | 是什么 | 怎么更新 |
| --- | --- | --- |
| `three.bundle.min.js` | three.js **r186.1** + OrbitControls，打成一个 min 过的 ES 模块（约 744 KB） | `node scripts/vendor-three.mjs [版本]` |
| `three.LICENSE.txt` | three.js 的 MIT 许可原文 | 跟着上面的脚本一起更新 |

页面里的用法（`/_vendor/` 是固定前缀，页面只能读到自己目录 + 这里）：

```html
<script type="module">
  import { Scene, PerspectiveCamera, WebGLRenderer, OrbitControls } from "/_vendor/three.js";
</script>
```

约定：**只预置这一个**。其它库（d3 / chart.js / katex…）需要联网就直说——把它们也搬进来
会让资产目录变成半个 CDN，升级维护的成本远大于收益；确实要用，页面里写 CDN 地址，
提示词里已经说明「断网时会白屏」。
