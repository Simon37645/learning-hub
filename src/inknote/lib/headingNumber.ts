/**
 * 标题编号的计算（对应 Typora LaTeX 主题里的 `heading-h2…h6` 计数器）。
 *
 * 规则与主题的 `counter-increment` / `counter-set` 逐条对齐：
 * - H1 是文档标题，本身不编号，并把所有计数器清零；
 * - H2 起每级各自递增，同时把更深的级别清零；
 * - 显示形式是「从 H2 到当前级」的编号用 `.` 连接，所以 H3 是 `1.1`、H4 是 `1.1.1`。
 */

export type HeadingCounters = number[];

/** 计数器数组，下标即标题级别（只用 2..6）。 */
export function createHeadingCounters(): HeadingCounters {
  return [0, 0, 0, 0, 0, 0, 0];
}

/**
 * 推进到下一个该级别的标题，返回它的编号文本；返回 `null` 表示这一级不显示编号（H1）。
 *
 * 就地修改传入的计数器，方便在语法树的单次顺序遍历里持续累计。
 */
export function advanceHeadingNumber(
  counters: HeadingCounters,
  level: number,
): string | null {
  if (level <= 1) {
    counters.fill(0);
    return null;
  }
  const safeLevel = Math.min(level, 6);
  counters[safeLevel] += 1;
  for (let i = safeLevel + 1; i <= 6; i += 1) counters[i] = 0;
  return counters.slice(2, safeLevel + 1).join(".");
}

/** 大纲等「已有标题序列」的场景：一次算出全部编号。 */
export function numberHeadings(levels: number[]): (string | null)[] {
  const counters = createHeadingCounters();
  return levels.map((level) => advanceHeadingNumber(counters, level));
}
