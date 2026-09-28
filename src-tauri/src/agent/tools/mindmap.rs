//! 思维导图：把层级结构变成一张 Mermaid 图，并存成一篇笔记。
//!
//! 为什么用 Mermaid 而不是自造一套节点数据：
//! - 它**就是纯文本**，落在 `.md` 里可以直接手改，换任何工具都能看
//! - 内置编辑器（InkNote 内核）与内置浏览器都已经能渲染 mermaid
//! - 需要流程图 / 时序图 / 时间线时，同一套机制就能覆盖，不用再加一种格式
//!
//! 节点文本统一写成 `id["文本"]`，这样即使文本里有括号、冒号、中文标点也不会把语法写坏。

use crate::agent::registry::{
    arg_str, arg_str_req, object_schema, str_prop, Tool, ToolCtx, ToolOutput,
};
use crate::error::{AppError, AppResult};
use crate::paths::ensure_dir;
use crate::store;
use async_trait::async_trait;
use serde_json::{json, Value};

/// 图太大会让 mermaid 渲染变慢、也看不清，超过就截断并告知
const MAX_NODES: usize = 120;
const MAX_DEPTH: usize = 6;

pub struct MindmapCreate;

#[async_trait]
impl Tool for MindmapCreate {
    fn name(&self) -> &'static str {
        "mindmap_create"
    }

    fn description(&self) -> &'static str {
        "创作一张思维导图（Mermaid mindmap）。两种输入方式，任选其一：\n\
         - `outline`：缩进文本，每行一个节点，用两个空格表示下一层（最省事，推荐）\n\
         - `nodes`：嵌套 JSON `[{text, children:[...]}]`\n\
         它会：① 存成一篇笔记 `notes/<标题>.md`（内置编辑器与内置浏览器都会渲染成图）\
         ② 在回复里返回图定义，你可以直接贴进回复，用户会在对话里看到图。\n\
         用途：梳理知识框架、对比概念、展示分类层级。预习阶段给全局地图尤其合适。\n\
         如果是要展示**流程 / 因果 / 时间线**，思维导图不合适，请改用 mermaid 的 \
         flowchart / sequenceDiagram / timeline 直接写进笔记。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "title": str_prop("导图标题，会成为文件名，例如「线性代数知识框架」"),
                "root": str_prop("中心主题（只有一个）。如果 outline 第一行就是中心，这里可留空"),
                "outline": str_prop("缩进大纲，每行一个节点，两个空格一层。例如：\\n心脏\\n  左心房\\n    肺静脉入口\\n  右心房"),
                "nodes": {
                    "type": "array",
                    "description": "嵌套结构（与 outline 二选一）：[{text, children:[...]}]",
                    "items": { "type": "object" }
                },
                "summary": str_prop("导图下面附一段 Markdown 说明（可选，例如「先看这三支」）"),
                "open": { "type": "boolean", "description": "是否立刻在内置浏览器打开，默认 true" },
                "topic": str_prop("写到另一个主题时传它的名字"),
            }),
            &["title"],
        )
    }

    fn risk(&self) -> crate::agent::event::Risk {
        crate::agent::event::Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "创作思维导图「{}」",
            arg_str(input, "title").unwrap_or_default()
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic_or(arg_str(&input, "topic").as_deref())?;
        let title = arg_str_req(&input, "title")?;
        let root = arg_str(&input, "root").unwrap_or_default();

        // 1) 收集成树
        let mut tree: Vec<Knot> = Vec::new();
        if let Some(outline) = arg_str(&input, "outline") {
            tree = parse_outline(&outline)?;
        } else if let Some(nodes) = input.get("nodes").and_then(|n| n.as_array()) {
            tree = parse_nodes(nodes);
        }
        if tree.is_empty() {
            return Err(AppError::invalid(
                "请给 outline（缩进大纲）或 nodes（嵌套结构）之一，至少要有一个节点",
            ));
        }

        // 中心主题：显式给的优先，否则用大纲的第一行（它已被当作根节点）
        let center = if root.trim().is_empty() {
            if tree.len() == 1 {
                // 单根：把它当中心，其子节点作为分支
                let knot = tree.remove(0);
                let children = knot.children;
                let center = knot.text;
                tree = children;
                center
            } else {
                title.clone()
            }
        } else {
            root
        };

        let (mermaid, nodes, truncated) = build_mermaid(&center, &tree)?;

        // 2) 存成笔记
        let mut body = String::new();
        body.push_str(&format!("---\ntitle: {title}\ntags: 思维导图\n---\n\n"));
        body.push_str(&format!("# {title}\n\n"));
        if let Some(summary) = arg_str(&input, "summary") {
            if !summary.trim().is_empty() {
                body.push_str(summary.trim());
                body.push_str("\n\n");
            }
        }
        body.push_str("```mermaid\n");
        body.push_str(&mermaid);
        body.push_str("\n```\n");
        if truncated {
            body.push_str(&format!(
                "\n> 节点太多，只画了前 {MAX_NODES} 个（层级最深 {MAX_DEPTH} 层）。\n"
            ));
        }

        let notes_dir = topic.notes_dir();
        ensure_dir(&notes_dir)?;
        let file_name = format!("{}.md", crate::paths::sanitize_dir_name(&title));
        let path = notes_dir.join(&file_name);
        let existed = path.exists();
        if existed {
            // 不覆盖用户可能已经改过的内容，另存一份
            let alt = crate::paths::unique_child(&notes_dir, &file_name);
            store::atomic_write(&alt, body.as_bytes())?;
            return finish(ctx, &topic, &alt, mermaid, nodes, truncated, true, input.get("open")).await;
        }
        store::atomic_write(&path, body.as_bytes())?;
        ctx.core.emit_topics_updated(&topic);
        finish(ctx, &topic, &path, mermaid, nodes, truncated, false, input.get("open")).await
    }
}

#[allow(clippy::too_many_arguments)]
async fn finish(
    ctx: &ToolCtx,
    topic: &crate::domain::topic::Topic,
    path: &std::path::Path,
    mermaid: String,
    nodes: usize,
    truncated: bool,
    renamed: bool,
    open: Option<&Value>,
) -> AppResult<ToolOutput> {
    let rel = topic.rel(path);
    let should_open = open.and_then(|v| v.as_bool()).unwrap_or(true);
    if should_open {
        let tab = ctx
            .core
            .viewer
            .open(crate::viewer::OpenRequest {
                url: None,
                path: Some(rel.clone()),
                topic_slug: Some(topic.slug()),
                title: Some(format!(
                    "{}（思维导图）",
                    path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
                )),
                kind: None,
                page: None,
                new_tab: false,
            })
            .await;
        if let Ok(tab) = tab {
            ctx.core
                .viewer
                .report_snapshot(&tab.id, Some(mermaid.clone()), None, None, None)
                .await;
        }
        ctx.core.emit_viewer_sync().await;
    }

    let mut out = String::new();
    out.push_str(&format!(
        "已生成思维导图（{nodes} 个节点{}）：{rel}\n\n",
        if truncated { "，已截断" } else { "" }
    ));
    if renamed {
        out.push_str("（同名笔记已存在，这份存成了新文件，没有覆盖原有内容）\n\n");
    }
    out.push_str("笔记内容（编辑器与内置浏览器会直接渲染成图）：\n\n```mermaid\n");
    out.push_str(&mermaid);
    out.push_str("\n```\n\n");
    out.push_str(
        "你可以把上面的 ```mermaid 代码块原样贴进回复里，用户会在对话中看到这张图。\
         讲的时候按图上的分支一条条走，不要一次讲完整张图。",
    );
    Ok(ToolOutput::ok(out))
}

// ---------------------------------------------------------------- 结构解析

struct Knot {
    text: String,
    children: Vec<Knot>,
}

/// 解析缩进大纲：两个空格（或一个 Tab）算一层。
///
/// 做法是先收集成 (缩进, 文本) 列表，再递归下降建树——
/// 比用栈加「父节点路径」去改借用中的子节点清楚得多。
fn parse_outline(text: &str) -> AppResult<Vec<Knot>> {
    let mut items: Vec<(usize, String)> = Vec::new();
    for raw in text.lines() {
        if raw.trim().is_empty() {
            continue;
        }
        let indent = raw
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .map(|c| if c == '\t' { 2 } else { 1 })
            .sum::<usize>();
        let text = raw
            .trim()
            .trim_start_matches(['-', '*', '+', '•', '·'])
            .trim()
            .to_string();
        if !text.is_empty() {
            items.push((indent, text));
        }
    }
    if items.is_empty() {
        return Err(AppError::invalid("outline 里没有解析出任何节点"));
    }

    /// 从 pos 开始，取所有缩进 >= base 的连续项，作为同一层的兄弟节点。
    fn parse(items: &[(usize, String)], pos: &mut usize, base: usize) -> Vec<Knot> {
        let mut out = Vec::new();
        while *pos < items.len() {
            let (indent, text) = &items[*pos];
            if *indent < base {
                break;
            }
            let this = *indent;
            *pos += 1;
            // 比当前更深的行都属于它的子节点
            let children = parse(items, pos, this + 1);
            out.push(Knot {
                text: text.clone(),
                children,
            });
        }
        out
    }

    let mut pos = 0;
    let roots = parse(&items, &mut pos, 0);
    if roots.is_empty() {
        return Err(AppError::invalid("outline 里没有解析出任何节点"));
    }
    Ok(roots)
}

fn parse_nodes(nodes: &[Value]) -> Vec<Knot> {
    nodes
        .iter()
        .filter_map(|n| {
            let text = n.get("text").and_then(|t| t.as_str())?.trim().to_string();
            if text.is_empty() {
                return None;
            }
            let children = n
                .get("children")
                .and_then(|c| c.as_array())
                .map(|c| parse_nodes(c))
                .unwrap_or_default();
            Some(Knot { text, children })
        })
        .collect()
}

// ---------------------------------------------------------------- 生成 Mermaid

/// 生成 `mindmap` 图。返回（图定义, 节点数, 是否被截断）。
fn build_mermaid(center: &str, branches: &[Knot]) -> AppResult<(String, usize, bool)> {
    let mut out = String::from("mindmap\n");
    let mut counter = 0usize;
    let mut truncated = false;

    out.push_str(&format!("  {}\n", node(&mut counter, center)));
    counter += 1;

    fn walk(out: &mut String, knots: &[Knot], depth: usize, counter: &mut usize, truncated: &mut bool) {
        for k in knots {
            if *counter >= MAX_NODES {
                *truncated = true;
                return;
            }
            // mermaid 用缩进表示层级：根下每层 4 空格，多个根时保持一致
            out.push_str(&" ".repeat(depth * 4));
            out.push_str(&node(counter, &k.text));
            out.push('\n');
            *counter += 1;
            if depth < MAX_DEPTH {
                walk(out, &k.children, depth + 1, counter, truncated);
            } else if !k.children.is_empty() {
                *truncated = true;
            }
        }
    }
    walk(&mut out, branches, 1, &mut counter, &mut truncated);

    if counter <= 1 {
        return Err(AppError::invalid("除中心主题外至少需要一个分支"));
    }
    Ok((out, counter, truncated))
}

/// 节点文本统一转义成 `id["文本"]`，避免括号/冒号/中文标点破坏语法。
fn node(counter: &mut usize, text: &str) -> String {
    let id = format!("n{}", *counter);
    let safe = text
        .replace('"', "”") // 双引号会截断字符串，换成中文引号
        .replace('\n', " ")
        .trim()
        .to_string();
    format!("{id}[\"{safe}\"]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_mindmap_from_outline() {
        let outline = "记忆\n  短时记忆\n    容量 7±2\n  长时记忆\n注意";
        let knots = parse_outline(outline).unwrap();
        assert_eq!(knots.len(), 2);
        assert_eq!(knots[0].text, "记忆");
        assert_eq!(knots[0].children.len(), 2);
        assert_eq!(knots[0].children[0].children[0].text, "容量 7±2");
        assert_eq!(knots[1].text, "注意");
    }

    #[test]
    fn escapes_quotes_and_wraps_nodes() {
        let n = node(&mut 0, "说\"引号\"");
        assert!(n.starts_with("n0[\""));
        assert!(!n.contains("说\"引号\""));

        let (mmd, count, truncated) = build_mermaid("中心", &[Knot { text: "分支".into(), children: vec![] }]).unwrap();
        assert!(mmd.starts_with("mindmap\n"));
        assert!(mmd.contains("n0[\"中心\"]"));
        assert!(mmd.contains("n1[\"分支\"]"));
        assert_eq!(count, 2);
        assert!(!truncated);
    }

    #[test]
    fn truncates_huge_maps() {
        let many: Vec<Knot> = (0..200)
            .map(|i| Knot { text: format!("节点{i}"), children: vec![] })
            .collect();
        let (_mmd, count, truncated) = build_mermaid("中心", &many).unwrap();
        assert!(truncated);
        assert!(count <= MAX_NODES + 1);
    }
}
