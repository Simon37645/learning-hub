//! 自定义外观主题：用户写一份 JSON，覆盖界面配色里的 CSS 变量。
//!
//! ## 为什么是「覆盖 CSS 变量」而不是「让用户写 CSS」
//!
//! 界面上的每一处颜色都走 `app.css` 里的 CSS 变量（`--bg` / `--text` / `--accent` …），
//! 所以自定义主题只需要给出「这些变量改成什么」：
//!
//! 1. 主题不可能把界面弄坏——变量名写错了，那个位置就沿用内置配色；
//! 2. 不需要注入样式表，也不用 eval 任何东西：前端用 `style.setProperty` 逐个设；
//! 3. 内置的两套配色（明亮/深色）仍然是底盘：`base` 决定跟随哪一套，
//!    用户只写自己关心的那几个颜色，其余照旧。
//!
//! ## 文件位置
//!
//! ```text
//! <工作区>/.hub/themes/<id>.json     跟着工作区走，可以进版本库
//! ~/.learning-hub/themes/<id>.json   跟着用户走，换工作区也带着
//! ```
//!
//! 同名时**工作区里的胜出**（`discover` 先扫工作区）——和技能目录的规则一致：
//! 越贴近当前工作区的越具体。
//!
//! ## 格式
//!
//! ```json
//! {
//!   "id": "solarized",
//!   "name": "Solarized",
//!   "description": "低对比度的护眼配色",
//!   "author": "我",
//!   "base": "dark",
//!   "vars": { "--bg": "#002b36", "--text": "#93a1a1" }
//! }
//! ```
//!
//! `id` 可省（默认取文件名）。`base` 是 `light` 或 `dark`。

use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 工作区里的主题目录（工作区相对路径）。
pub const DIR_REL: &str = ".hub/themes";
/// 用户目录里的主题目录（相对家目录）。
pub const USER_DIR: &str = ".learning-hub/themes";

/// 主题自带哪套内置配色作为底盘。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ThemeBase {
    #[default]
    Light,
    Dark,
}

impl ThemeBase {
    pub fn as_str(self) -> &'static str {
        match self {
            ThemeBase::Light => "light",
            ThemeBase::Dark => "dark",
        }
    }
}

/// 一份自定义主题。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Theme {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub base: ThemeBase,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// 「工作区」/「用户目录」——界面上要区分这两类
    #[serde(default)]
    pub source: String,
    /// 文件绝对路径
    #[serde(default)]
    pub path: String,
    /// 这个文件读不了时给出原因（仍然列出来：让用户看见自己的文件坏了，
    /// 而不是「我写的主题没出现，是不是软件坏了？」）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Theme {
    pub fn usable(&self) -> bool {
        self.error.is_none() && !self.vars.is_empty()
    }
}

/// 界面上「可用的变量」帮助列表。
///
/// **这份列表必须和 `app.css` 里的定义一致**——底下有个单测直接解析 `app.css`
/// 并对齐两边，加/删变量忘了改这里会红。这是刻意的：文档漂了比没有文档更糟。
pub const VAR_DOCS: &[(&str, &str)] = &[
    ("--bg", "主背景"),
    ("--bg-sub", "次级背景（卡片、输入框）"),
    ("--bg-side", "侧栏背景"),
    ("--bg-hover", "悬停底色"),
    ("--bg-active", "选中底色"),
    ("--bg-inverse", "反色块（徽标、代码行号）"),
    ("--bg-code", "代码块背景"),
    ("--border", "分隔线与边框"),
    ("--border-strong", "加重一点的边框"),
    ("--text", "正文"),
    ("--text-sub", "次要文字"),
    ("--text-faint", "最浅的文字（时间、占位）"),
    ("--text-inverse", "反色块上的文字"),
    ("--accent", "强调色（按钮、链接、选中态）"),
    ("--accent-soft", "强调色的淡底"),
    ("--accent-border", "强调色的描边"),
    ("--ok", "成功 / 正确"),
    ("--ok-soft", "成功色的淡底"),
    ("--warn", "提醒"),
    ("--warn-soft", "提醒色的淡底"),
    ("--danger", "错误 / 危险"),
    ("--danger-soft", "错误色的淡底"),
    ("--radius-sm", "小圆角"),
    ("--radius", "圆角"),
    ("--radius-lg", "大圆角"),
    ("--sidebar-w", "侧栏默认宽度"),
    ("--shadow-sm", "小投影"),
    ("--shadow", "投影"),
    ("--shadow-lg", "大投影"),
    ("--font", "正文字体"),
    ("--font-mono", "等宽字体"),
    ("--code-keyword", "代码：关键字"),
    ("--code-string", "代码：字符串"),
    ("--code-number", "代码：数字"),
    ("--code-title", "代码：标题 / 函数名"),
    ("--code-builtin", "代码：内置对象"),
    ("--speed", "过渡时长"),
];

/// 变量名的形状限制。值不校验「是不是合法颜色」——那会挡掉
/// `color-mix(...)`、`var(--x)` 这类完全合法的写法，而写错了浏览器只会忽略这一条。
fn valid_var_name(name: &str) -> bool {
    name.starts_with("--")
        && name.len() <= 64
        && name[2..]
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-')
        && name.len() > 2
}

/// 值里不许出现能拼出「另一条声明」的字符。用 `setProperty` 时浏览器本来就会拒绝
/// 畸形的值，这里再挡一道是为了让**主题文件本身**保持可读、可 diff。
fn valid_var_value(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && !value.contains([';', '{', '}', '<', '>'])
        && !value.contains('\n')
}

/// 过滤掉不合法的变量（保留合法的，不因为一个笔误让整个主题失效）。
fn sanitize_vars(raw: BTreeMap<String, String>) -> BTreeMap<String, String> {
    raw.into_iter()
        .filter(|(k, v)| valid_var_name(k) && valid_var_value(v))
        .take(200)
        .collect()
}

/// 从一份 JSON 文本解析主题。`fallback_id` 是文件名去后缀（JSON 里没写 id 时用它）。
pub fn parse(id_hint: &str, text: &str) -> Result<Theme, String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        description: String,
        #[serde(default)]
        author: String,
        #[serde(default)]
        base: ThemeBase,
        #[serde(default)]
        vars: BTreeMap<String, String>,
    }
    let raw: Raw = serde_json::from_str(text).map_err(|e| format!("不是合法的 JSON：{e}"))?;
    let id = if raw.id.trim().is_empty() {
        id_hint.to_string()
    } else {
        raw.id.trim().to_string()
    };
    if id.trim().is_empty() {
        return Err("没有 id（也没法从文件名推断）".into());
    }
    let vars = sanitize_vars(raw.vars);
    Ok(Theme {
        name: if raw.name.trim().is_empty() { id.clone() } else { raw.name.trim().to_string() },
        id,
        description: raw.description,
        author: raw.author,
        base: raw.base,
        vars,
        source: String::new(),
        path: String::new(),
        error: None,
    })
}

/// 扫一个目录下的 `*.json`。读不了的文件也会出现在结果里（带 `error`）。
fn scan(dir: &Path, source: &str, out: &mut Vec<Theme>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !path.extension().is_some_and(|e| e == "json") {
            continue;
        }
        // 侧车/说明文件不该被当成主题
        let stem = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if stem.starts_with('.') {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(t) => t,
            Err(e) => {
                out.push(broken(&stem, source, &path, format!("读不了这个文件：{e}")));
                continue;
            }
        };
        match parse(&stem, &text) {
            Ok(mut theme) => {
                theme.source = source.to_string();
                theme.path = path.to_string_lossy().to_string();
                if theme.vars.is_empty() {
                    theme.error = Some("里面没有可用的变量（vars 是空的？变量名要写成 \"--bg\" 这种）".into());
                }
                push_unique(out, theme);
            }
            Err(msg) => out.push(broken(&stem, source, &path, msg)),
        }
    }
}

fn broken(id: &str, source: &str, path: &Path, msg: String) -> Theme {
    Theme {
        id: id.to_string(),
        name: id.to_string(),
        description: String::new(),
        author: String::new(),
        base: ThemeBase::Light,
        vars: BTreeMap::new(),
        source: source.to_string(),
        path: path.to_string_lossy().to_string(),
        error: Some(msg),
    }
}

/// 同名主题：先扫到的胜出（工作区先扫）。
fn push_unique(out: &mut Vec<Theme>, theme: Theme) {
    if !out.iter().any(|t| t.id == theme.id) {
        out.push(theme);
    }
}

/// 找出所有可用主题。顺序即优先级：工作区 → 用户目录。
pub fn discover(workspace_root: &Path) -> Vec<Theme> {
    let mut out: Vec<Theme> = Vec::new();
    scan(&workspace_root.join(".hub").join("themes"), "工作区", &mut out);
    if let Some(home) = crate::skills::home_dir() {
        scan(&home.join(".learning-hub").join("themes"), "用户目录", &mut out);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// 按 id 找一个**可用**的主题（读不了的、找不到的都当没有：
/// 界面退回内置配色是这里唯一安全的默认行为）。
pub fn find(themes: &[Theme], id: &str) -> Option<Theme> {
    themes.iter().find(|t| t.id == id && t.usable()).cloned()
}

/// 工作区主题目录（写新主题、打开目录都用它）。
pub fn workspace_dir(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".hub").join("themes")
}

/// 保存一份主题到工作区主题目录。`id` 来自文件名/内容，覆盖同 id 的文件。
pub fn save(workspace_root: &Path, theme: &Theme) -> AppResult<PathBuf> {
    let id = crate::studio::sanitize_id(&theme.id)?;
    let dir = workspace_dir(workspace_root);
    crate::paths::ensure_dir(&dir)?;
    let path = dir.join(format!("{id}.json"));
    if theme.vars.is_empty() {
        return Err(AppError::invalid("主题里至少要有一个变量，否则它什么也改不了"));
    }
    let payload = Theme {
        id: id.clone(),
        name: if theme.name.trim().is_empty() { id.clone() } else { theme.name.trim().to_string() },
        description: theme.description.trim().to_string(),
        author: theme.author.trim().to_string(),
        base: theme.base,
        vars: sanitize_vars(theme.vars.clone()),
        source: "工作区".into(),
        path: path.to_string_lossy().to_string(),
        error: None,
    };
    crate::store::write_json(&path, &payload)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// **这份文档列表必须和 app.css 一致**：加/删了变量而忘了改 `VAR_DOCS`，
    /// 界面上那份「可用的变量」就成了假文档（比没有更糟）。
    #[test]
    fn var_docs_match_app_css() {
        let css = include_str!("../../src/styles/app.css");
        // 只解析 `:root { … }` 与 `html[data-theme="dark"] { … }` 两个块
        let block = |selector: &str| -> Vec<String> {
            let at = css.find(selector).expect("找不到样式块") + selector.len();
            let rest = &css[at..];
            let end = rest.find("\n}").expect("样式块没有结束");
            rest[..end]
                .lines()
                .filter_map(|l| {
                    let name = l.trim().strip_prefix("--")?;
                    let (name, _) = name.split_once(':')?;
                    Some(format!("--{}", name.trim()))
                })
                .collect()
        };

        let root_vars = block(":root {");
        let docs: Vec<String> = VAR_DOCS.iter().map(|(n, _)| n.to_string()).collect();

        for v in &root_vars {
            assert!(
                docs.contains(v),
                "{v} 在 app.css 里有、却没写进 VAR_DOCS（主题文档会漏掉它）"
            );
        }
        for d in &docs {
            assert!(
                root_vars.contains(d),
                "{d} 在 VAR_DOCS 里、却不在 app.css 的 :root 里（文档漂了）"
            );
        }
        // 深色块只允许重定义 :root 里已有的变量（多出一个就说明有人写错了名字）
        let dark = block("html[data-theme=\"dark\"] {");
        for v in &dark {
            assert!(root_vars.contains(v), "深色块定义了 :root 里没有的 {v}");
        }
        assert!(dark.len() > 10, "深色块应当重定义大部分颜色变量");
    }

    #[test]
    fn parses_minimal_and_full_files() {
        let t = parse("sunset", r##"{ "vars": { "--bg": "#fff" } }"##).unwrap();
        assert_eq!(t.id, "sunset", "没写 id 时用文件名");
        assert_eq!(t.name, "sunset");
        assert_eq!(t.base, ThemeBase::Light);
        assert!(t.usable());

        let t = parse(
            "anything",
            r##"{
                "id": "solarized",
                "name": "Solarized",
                "description": "护眼",
                "author": "我",
                "base": "dark",
                "vars": { "--bg": "#002b36", "--text": "color-mix(in srgb, white 80%, black)" }
            }"##,
        )
        .unwrap();
        assert_eq!(t.id, "solarized");
        assert_eq!(t.base, ThemeBase::Dark);
        assert_eq!(t.vars.len(), 2, "color-mix(...) 是合法写法，不该被过滤");

        // 不是 JSON / 空文件 → 明确的错误，而不是静默失败
        assert!(parse("x", "{ 这不是 json").is_err());
        assert!(parse("x", "").is_err());
    }

    /// 变量名与值都要挡住能拼出「另一条声明」的东西（剩下的只是笔误，浏览器会忽略）。
    #[test]
    fn sanitizes_var_names_and_values() {
        let mut vars = BTreeMap::new();
        vars.insert("--bg".to_string(), "#fff".to_string());
        vars.insert("bg".to_string(), "#fff".to_string()); // 少了 --：忽略
        vars.insert("--x".to_string(), "red; } body { display: none".to_string()); // 注入尝试
        vars.insert("--y".to_string(), "url('a')".to_string()); // 合法
        vars.insert("--z".to_string(), "".to_string()); // 空值：忽略
        vars.insert("--a b".to_string(), "red".to_string()); // 名字带空格：忽略
        let clean = sanitize_vars(vars);
        assert!(clean.contains_key("--bg"));
        assert!(clean.contains_key("--y"));
        assert!(!clean.contains_key("bg"));
        assert!(!clean.contains_key("--x"));
        assert!(!clean.contains_key("--z"));
        assert!(!clean.contains_key("--a b"));
    }

    /// 工作区里的同名主题胜出；用户目录里的也能看到；坏文件带原因列出来。
    #[test]
    fn discovers_workspace_first_and_keeps_broken_files_visible() {
        let tmp = std::env::temp_dir().join(format!("lh-theme-{}", uuid::Uuid::new_v4()));
        let ws = tmp.join("工作区");
        let dir = workspace_dir(&ws);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("sunset.json"), r##"{"name":"工作区的日落","vars":{"--bg":"#f60"}}"##).unwrap();
        std::fs::write(dir.join("broken.json"), "{ 半截").unwrap();
        // 非 JSON 与隐藏文件都要跳过
        std::fs::write(dir.join("notes.txt"), "不是主题").unwrap();
        std::fs::write(dir.join(".hidden.json"), r##"{"vars":{"--bg":"#000"}}"##).unwrap();

        let themes = discover(&ws);
        let ids: Vec<&str> = themes.iter().map(|t| t.id.as_str()).collect();
        assert!(ids.contains(&"sunset"));
        assert!(ids.contains(&"broken"), "读不了的文件也要列出来");
        assert!(!ids.contains(&"notes"), "只有 .json 才是主题");
        assert!(!ids.contains(&".hidden"));
        let broken = themes.iter().find(|t| t.id == "broken").unwrap();
        assert!(broken.error.is_some() && !broken.usable());

        let sunset = find(&themes, "sunset").expect("应当能按 id 找到");
        assert_eq!(sunset.name, "工作区的日落");
        assert_eq!(sunset.base, ThemeBase::Light);
        assert_eq!(sunset.vars.get("--bg").map(String::as_str), Some("#f60"));
        // 读不了的不算数：宁可退回内置配色
        assert!(find(&themes, "broken").is_none());
        assert!(find(&themes, "不存在").is_none());

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn save_roundtrip_and_guards() {
        let tmp = std::env::temp_dir().join(format!("lh-theme-{}", uuid::Uuid::new_v4()));
        let mut theme = Theme {
            id: "我的 主题".into(),
            name: "我的主题".into(),
            description: "  ".into(),
            author: "我".into(),
            base: ThemeBase::Dark,
            vars: BTreeMap::from([("--bg".to_string(), "#123".to_string())]),
            source: String::new(),
            path: String::new(),
            error: None,
        };
        let path = save(&tmp, &theme).unwrap();
        assert_eq!(path.file_name().unwrap(), "我的 主题.json");
        let back = discover(&tmp);
        let t = find(&back, "我的 主题").expect("存进去的要能读回来");
        assert_eq!(t.base, ThemeBase::Dark);
        assert_eq!(t.description, "");

        // 一个变量都没有的主题没有意义
        theme.vars.clear();
        assert!(save(&tmp, &theme).is_err());
        // id 不能越界
        theme.id = "../escape".into();
        theme.vars = BTreeMap::from([("--bg".to_string(), "#123".to_string())]);
        assert!(save(&tmp, &theme).is_err());

        std::fs::remove_dir_all(&tmp).ok();
    }
}
