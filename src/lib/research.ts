// 深度研究（DeepResearch）模式的纯逻辑：触发前缀的解析与报告标题的提取。
//
// 触发方式是输入框里的「/研究 」前缀（对标 GPT 的 Deep Research 的单轮开关）：
// 选前缀而不是 Composer 上的常驻模式开关，是因为研究是「一轮」的属性——
// 前缀天然跟着那条消息走、不需要额外状态，也不用在界面上多一个要记得关的开关。
// 解析放在 store.send 里做（所有发送路径——直接发 / 排队发 / 打断重发——都汇经那里），
// 组件里只拿它做「下一轮是深度研究」的即时提示。

/** 触发深度研究的前缀。中文界面中文命令；与 README / 提示里的写法保持一致 */
export const RESEARCH_PREFIX = "/研究";

export interface ParsedResearchInput {
  /** 这一条是不是深度研究轮 */
  deep: boolean;
  /** 去掉前缀后的问题文本（可能为空串；空串 + 无图会被发送方拒掉） */
  text: string;
}

/**
 * 解析输入框里的深度研究前缀。
 *
 * 规则：整条消息（已 trim）以「/研究」开头，且后面要么结束、要么是空白才算触发——
 * 「/研究方法」这种正好在聊研究方法本身的消息不触发。
 * 触发时去掉前缀与其后的空白，剩下的才是真正要研究的问题。
 */
export function parseResearchInput(raw: string): ParsedResearchInput {
  const t = raw.trim();
  if (!t.startsWith(RESEARCH_PREFIX)) return { deep: false, text: t };
  const rest = t.slice(RESEARCH_PREFIX.length);
  if (rest.length > 0 && !/^\s/.test(rest)) return { deep: false, text: t };
  return { deep: true, text: rest.trim() };
}

/**
 * 从报告正文里取一个能当笔记标题的短句：
 * 优先第一个 Markdown 标题（# 到 ###### 都认），退而求其次把正文压成一行截前 24 个字符。
 * 名字最终会过 Rust 侧 `note_create` 的 sanitize_dir_name，这里不用操心文件名合法性。
 */
export function researchReportTitle(text: string): string {
  for (const line of text.split(/\r?\n/)) {
    const m = /^#{1,6}\s+(.+?)\s*$/.exec(line);
    if (m) return m[1];
  }
  const flat = text.replace(/\s+/g, " ").trim();
  if (!flat) return "深度研究";
  return flat.length > 24 ? flat.slice(0, 24) : flat;
}
