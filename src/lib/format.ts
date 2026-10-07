// 展示层的小工具：时间、体积、文本处理。

/** 相对时间：刚刚 / 3 分钟前 / 昨天 / 10-01 */
export function relTime(iso?: string | null): string {
  if (!iso) return "";
  const t = new Date(iso).getTime();
  if (Number.isNaN(t)) return "";
  const diff = Date.now() - t;
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const hour = Math.floor(min / 60);
  if (hour < 24) return `${hour} 小时前`;
  const day = Math.floor(hour / 24);
  if (day === 1) return "昨天";
  if (day < 7) return `${day} 天前`;
  return fmtDate(iso);
}

export function fmtDate(iso?: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const now = new Date();
  const sameYear = d.getFullYear() === now.getFullYear();
  const p = (n: number) => String(n).padStart(2, "0");
  return sameYear
    ? `${d.getMonth() + 1}-${p(d.getDate())}`
    : `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

export function fmtDateTime(iso?: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getMonth() + 1}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function fmtClock(iso?: string | null): string {
  if (!iso) return "";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getHours())}:${p(d.getMinutes())}`;
}

/** 距今天数：负数=已逾期 */
export function daysFromToday(iso?: string | null): number | null {
  if (!iso) return null;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  const a = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const now = new Date();
  const b = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  return Math.round((a - b) / 86400000);
}

export function dueLabel(iso?: string | null): string {
  const d = daysFromToday(iso);
  if (d === null) return "";
  if (d < 0) return `逾期 ${-d} 天`;
  if (d === 0) return "今天";
  if (d === 1) return "明天";
  if (d <= 7) return `${d} 天后`;
  return fmtDate(iso);
}

export function humanBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}

export function clampText(s: string, n: number): string {
  return s.length <= n ? s : `${s.slice(0, n)}…`;
}

/** 粗略估算 token（中文 1 字≈1 token，英文 4 字符≈1） */
export function estimateTokens(s: string): number {
  let cjk = 0;
  let other = 0;
  for (const ch of s) {
    const code = ch.codePointAt(0) ?? 0;
    if (code >= 0x4e00 && code <= 0x9fff) cjk++;
    else other++;
  }
  return cjk + Math.ceil(other / 4);
}

/** token 数的中文写法：24.9万 / 8.2k / 640（用量面板上比 249000 好读） */
export function fmtTokens(n: number): string {
  if (n >= 10000) return `${(n / 10000).toFixed(1)}万`;
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`;
  return String(n);
}

export function isMac(): boolean {
  return navigator.platform.toLowerCase().includes("mac");
}

/** 快捷键展示：⌘K / Ctrl+K */
export function hotkey(key: string): string {
  return isMac() ? `⌘${key}` : `Ctrl+${key}`;
}
