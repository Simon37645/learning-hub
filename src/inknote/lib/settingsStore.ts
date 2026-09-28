// 设置存取（移植自 InkNote，持久化后端替换为 localStorage）。
//
// 原实现把设置存进 InkNote 自己的 `load_app_settings` / `save_app_settings` 命令；
// 学习中枢的配置由 `config.json`（AppConfig）管，编辑器偏好这类「界面小事」
// 没必要占用后端配置，直接落在 localStorage 更简单，也不会污染用户的配置。

const PREFIX = "hub.editor.";

let cache = new Map<string, string>();
let initialized = false;
let flushTimer: ReturnType<typeof setTimeout> | null = null;

function storageKey(key: string): string {
  // InkNote 的键名（mdnote.* / inknote.*）原样保留在 localStorage 里，
  // 方便和上游对照；前缀把它圈进自己的命名空间。
  return PREFIX + key;
}

function safeStorage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

export async function initializeSettingsStore() {
  if (initialized) return;
  initialized = true;
  const store = safeStorage();
  if (!store) return;
  for (let i = 0; i < store.length; i++) {
    const k = store.key(i);
    if (k && k.startsWith(PREFIX)) {
      const v = store.getItem(k);
      if (v !== null) cache.set(k.slice(PREFIX.length), v);
    }
  }
}

export function getStoredValue(key: string): string | null {
  const k = storageKey(key);
  if (cache.has(k)) return cache.get(k) ?? null;
  const v = safeStorage()?.getItem(k) ?? null;
  if (v !== null) cache.set(k, v);
  return v;
}

export function setStoredValue(key: string, value: string) {
  cache.set(storageKey(key), value);
  scheduleFlush();
}

export function removeStoredValue(key: string) {
  cache.delete(storageKey(key));
  scheduleFlush();
}

/** 落盘节流：编辑器里有字号/行高这类连续调整，不该每帧都写 localStorage */
function scheduleFlush() {
  if (flushTimer) clearTimeout(flushTimer);
  flushTimer = setTimeout(() => void flushSettingsStore(), 120);
}

export async function flushSettingsStore(): Promise<void> {
  const store = safeStorage();
  flushTimer = null;
  if (!store) return;
  for (const [k, v] of cache) {
    try {
      store.setItem(PREFIX + k, v);
    } catch {
      // 配额满了就算了，界面设置不值得打断用户
    }
  }
}

export function resetSettingsStoreForTests() {
  cache = new Map();
  initialized = false;
}
