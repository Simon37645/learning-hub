// 用量统计：把「服务商报回来的真实 token 用量」折算成界面上要显示的数字。
//
// 为什么单独一个文件：对话框底部那枚「缓存 X%」胶囊是整个对话的累计值，
// 而明细浮层是上一轮的构成。两者都靠这里的纯函数算，便于单测。

import type { ChatMessage, ContextPart } from "./types";

export interface CacheSummary {
  /** 参与统计的输入 token 总量 */
  input: number;
  /** 其中命中缓存的部分 */
  cached: number;
  /** 参与统计的轮次数（只有报过用量的轮次算数） */
  turns: number;
  /** 命中率 0–100 */
  rate: number;
}

/**
 * 一条对话整体的缓存命中率。
 *
 * 只统计**服务商报过用量**的轮次（`cachedTokens` 不为 null）：
 * - 老对话（这个功能之前记的消息）没有缓存字段 → 不计入，也不显示
 * - 服务商不报用量（比如没开 stream_options 的 OpenAI）→ 不计入
 * 宁可少算几轮，也不要把「没数据」算成「没命中」，否则命中率会假性偏低。
 */
export function summarizeCache(messages: ChatMessage[]): CacheSummary {
  let input = 0;
  let cached = 0;
  let turns = 0;
  for (const m of messages) {
    const meta = m.meta;
    if (!meta || meta.cachedTokens == null || !meta.inputTokens) continue;
    input += meta.inputTokens;
    cached += meta.cachedTokens;
    turns += 1;
  }
  return { input, cached, turns, rate: input > 0 ? (cached / input) * 100 : 0 };
}

/** 上下文构成里各块占比（0–100）；总和不一定是 100（按字符估的，只用来画比例）。 */
export function contextShares(parts: ContextPart[]): { label: string; tokens: number; share: number }[] {
  const total = parts.reduce((n, p) => n + p.tokens, 0);
  if (total <= 0) return [];
  return parts.map((p) => ({ label: p.label, tokens: p.tokens, share: (p.tokens / total) * 100 }));
}
