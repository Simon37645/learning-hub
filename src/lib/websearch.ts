// 联网搜索（DeepSeek）的前端纯逻辑：条数输入的钳制。
//
// 后端会把 count 夹到 1-50（见 agent/tools/web.rs 的 build_search_body），
// 这里做同一套钳制，让设置页的数字输入框在写回配置之前就落进合法区间——
// 两边同一套规则，界面上不会出现「存了 99、面板显示 99、实际按 50 发」的错位。

/** DeepSeek 搜索单次结果的默认条数（按次计费，省着用） */
export const DEFAULT_SEARCH_COUNT = 8;

/** 把任意输入钳制成合法的结果条数（1-50）：缺失/空输入回默认值 8 */
export function clampSearchCount(raw: unknown): number {
  // 「没填」和「填了非法内容」是两回事：前者是缺数据（回默认），后者取不到数字也回默认
  if (raw === null || raw === undefined || raw === "") return DEFAULT_SEARCH_COUNT;
  const n = Number(raw);
  if (!Number.isFinite(n)) return DEFAULT_SEARCH_COUNT;
  return Math.min(COUNT_MAX, Math.max(COUNT_MIN, Math.round(n)));
}

export const COUNT_MIN = 1;
export const COUNT_MAX = 50;
