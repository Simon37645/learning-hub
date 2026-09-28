type MermaidTheme = "dark" | "default" | "neutral";
type MermaidApi = typeof import("mermaid")["default"];

let mermaidPromise: Promise<MermaidApi> | null = null;
let activeTheme: MermaidTheme | null = null;

/** Mermaid 体积较大，仅在文档实际包含图表时加载。 */
export async function configuredMermaid(theme: MermaidTheme): Promise<MermaidApi> {
  mermaidPromise ??= import("mermaid").then((module) => module.default);
  const mermaid = await mermaidPromise;
  if (activeTheme !== theme) {
    mermaid.initialize({
      startOnLoad: false,
      theme,
      securityLevel: "strict",
      // 解析失败时不要往 DOM 里塞那个「Syntax error in text」错误框：
      // 它是全局副作用，会一直留在界面上（甚至被截进截图），
      // 而调用方本来就会自己处理异常并保留原始代码块。
      suppressErrorRendering: true,
    });
    activeTheme = theme;
  }
  return mermaid;
}

/**
 * 渲染 id 的全局计数器。
 *
 * **必须全局唯一**：Mermaid 的 `render(id, ...)` 会用一个 id 形如 `d<id>` 的临时节点
 * 来排布图，两个调用方各自从 1 开始编号就会撞 id，撞上直接抛
 * 「Syntax error in text」——明明图是合法的。界面上有两个入口会渲染 Mermaid
 * （聊天/内置浏览器的 Markdown 渲染器，以及编辑器里的图表组件），
 * 所以计数器放在这个共享模块里，两边共用。
 */
let diagramSeq = 0;

/** 串行队列：Mermaid 的 render 不是并发安全的（共用 DOM 临时节点与内部状态），
 *  聊天和编辑器可能同时触发渲染，这里排成队列逐张画。 */
let renderChain: Promise<unknown> = Promise.resolve();

/**
 * 渲染一张 Mermaid 图，返回 SVG 源码。
 *
 * 这是全应用唯一的渲染入口：拉取实例、排队、分配唯一 id、错误抑制都在这里处理，
 * 调用方只需要处理「成功拿到 svg / 失败抛异常」。
 */
export async function renderDiagram(source: string, theme: MermaidTheme): Promise<string> {
  const run = async (): Promise<string> => {
    const mermaid = await configuredMermaid(theme);
    const id = `hub-mmd-${++diagramSeq}`;
    const { svg } = await mermaid.render(id, source);
    return svg;
  };
  // 前一张画完（无论成功失败）再画下一张，失败不能卡住队列
  const result = renderChain.then(run, run);
  renderChain = result.then(
    () => undefined,
    () => undefined,
  );
  return result;
}
