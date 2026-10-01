//! 长期记忆工具：让 agent **自己**决定「这件事以后还用得上」并记下来。
//!
//! 三条工具对应记忆的三个动作：
//! - `memory_write`  记一条 / 改一条（重复内容会自动合并，不会攒出一堆同义条）
//! - `memory_list`   看现在记了什么（写之前先看一眼，避免重复；用户问「你记得我什么」也用它）
//! - `memory_forget` 删一条（记错了、过时了，要能撤掉）
//!
//! 作用域只有两种：全局（跨主题都成立）与指定主题（含父主题继承）。
//! **不提供**「静默记一堆」的能力——每条记忆都写进用户能打开的文件，
//! 也都在界面的「记忆」面板里可见可改。
//!
//! 注入那一侧不需要工具：`AppCore::memory_digest` 每轮都会把相关记忆放进系统提示词。

use crate::agent::registry::{
    arg_bool, arg_str, arg_str_array, arg_str_req, object_schema, str_prop, Tool, ToolCtx, ToolOutput,
};
use crate::domain::memory::{Memory, MemoryKind, Scope as MemoryScope};
use crate::error::AppResult;use async_trait::async_trait;
use serde_json::{json, Value};

/// 写入/删除记忆要用户点头吗？——不用。
///
/// 记忆不是用户的资料，写错了在面板里一键就能删；如果每条都要确认，
/// 用户会被打断到直接关掉这个功能。真正危险的动作是删**文件**，那才需要审批。
pub struct MemoryWrite;

#[async_trait]
impl Tool for MemoryWrite {
    fn name(&self) -> &'static str {
        "memory_write"
    }

    fn description(&self) -> &'static str {
        "记一条**长期**记忆（跨对话、跨主题都有效），或修改已有的一条。\n\
         什么时候该记：\n\
         - 用户说了关于自己的稳定信息（专业、基础、在准备什么考试、身体/作息限制）；\n\
         - 用户表达了学习方式偏好（要例子不要公式、一次别讲太多、先给结论）；\n\
         - 用户反复出错、或明确说「我总把这个搞混」——记成注意（pitfall）；\n\
         - 你发现了他还没掌握的前置知识（gap），下次讲之前要先补。\n\
         什么时候**不要**记：一次性的问题、当前对话的临时约定、你自己推测出来的东西、\
         已经在「长期记忆」版块里出现过的内容（那会重复）。\n\
         scope：默认记进当前主题；只有确实跨主题成立的事（称呼、作息、通用偏好）才用 global。\n\
         已存在同义内容时会自动更新那一条，不会新增。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "content": str_prop("一句话说清这件事，要能独立读懂。例如「他把特征值和特征向量搞混，看到 Av=λv 会反过来认」"),
                "kind": str_prop("分类：fact(情况) / preference(偏好) / goal(目标) / pitfall(注意) / style(讲法) / gap(缺口)"),
                "scope": str_prop("global=跨主题通用；topic=只属于某个主题（默认当前主题）"),
                "topic": str_prop("写进哪个主题（只在 scope=topic 且不是当前主题时需要）"),
                "note": str_prop("可选的补充说明：当时的上下文、证据"),
                "source": str_prop("可选的来源，例如「2024-05 复习时的对话」"),
                "id": str_prop("要修改的那条记忆的 id（来自 memory_list）。给了它就是改而不是新增"),
                "pinned": { "type": "boolean", "description": "钉住：永远优先注入，适合「称呼」「总目标」这类" },
            }),
            &["content"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        let c: String = arg_str(input, "content").unwrap_or_default().chars().take(40).collect();
        match arg_str(input, "id") {
            Some(id) if !id.trim().is_empty() => format!("修改记忆 {id}：{c}"),
            _ => format!("记住：{c}"),
        }
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        if !ctx.core.memory_enabled() {
            return Ok(ToolOutput::err(
                "长期记忆功能现在是关闭的（设置 → Agent → 长期记忆）。\
                 需要记的话请先让用户打开它。",
            ));
        }
        let content = arg_str_req(&input, "content")?;
        // 分类没写或写错时按「情况」处理：宁可分类粗一点，也不要因为参数不合法把记忆丢掉
        let kind = arg_str(&input, "kind")
            .and_then(|s| MemoryKind::parse(&s))
            .unwrap_or(MemoryKind::Fact);
        let scope_word = arg_str(&input, "scope").unwrap_or_default();
        let to_global = matches!(scope_word.trim().to_ascii_lowercase().as_str(), "global" | "全局");

        // 记到哪个主题：显式给了 topic 就解析它，否则用当前主题；要求全局就不看主题
        let target = if to_global {
            None
        } else {
            let topic_arg = arg_str(&input, "topic");
            match topic_arg.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(s) => Some(ctx.core.config_read().workspace().resolve(s)?),
                None => match ctx.topic.as_ref() {
                    Some(t) => Some(t.clone()),
                    None => {
                        // 首页聊天没有主题：跨主题成立的事记全局，别硬塞进某个主题
                        None
                    }
                },
            }
        };
        let scope = match &target {
            Some(t) => MemoryScope::Topic(t.slug()),
            None => MemoryScope::Global,
        };
        let dir = target.as_ref().map(|t| t.dir.clone());

        let mut item = Memory::new(kind, content);
        item.note = arg_str(&input, "note").unwrap_or_default();
        item.source = arg_str(&input, "source");
        item.pinned = arg_bool(&input, "pinned").unwrap_or(false);

        let id = arg_str(&input, "id").unwrap_or_default();
        let id = id.trim().to_string();

        let (saved, added) = ctx.core.memory_write(|store| {
            if id.is_empty() {
                store.upsert(&scope, dir.as_deref(), item)
            } else {
                // 改：只动分类/内容/钉住/备注，其余字段（创建时间、使用次数）保持原样
                let updated = store.patch(
                    &scope,
                    dir.as_deref(),
                    &id,
                    Some(item.kind),
                    Some(item.content),
                    Some(item.pinned),
                    // 只有明确给了备注才覆盖，免得「改个分类」把原来的备注清掉
                    arg_str(&input, "note"),
                )?;
                Ok((updated, false))
            }
        })?;
        let where_text = match &target {
            Some(t) => format!("主题「{}」", t.meta.name),
            None => "全局（所有主题）".to_string(),
        };

        ctx.core.emit_memory_changed(&where_text);
        Ok(ToolOutput::ok(format!(
            "已{}记忆（{}，{}）：{}\nid={}\n\
             这条以后每轮都会随系统提示词给你，不用再重复记。",
            if added { "记下" } else { "更新" },
            saved.kind.label(),
            where_text,
            saved.content,
            saved.id
        )))
    }
}

pub struct MemoryList;

#[async_trait]
impl Tool for MemoryList {
    fn name(&self) -> &'static str {
        "memory_list"
    }

    fn description(&self) -> &'static str {
        "列出长期记忆（含当前主题继承自父主题的、以及全局的）。\n\
         写新记忆前先用它看一眼，避免重复；用户问「你还记得我什么」时也用它回答。\n\
         每条前面的 id 可以用来修改（memory_write 传 id）或删除（memory_forget）。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "kind": str_prop("只看某一类：fact / preference / goal / pitfall / style / gap"),
                "contains": str_prop("只列内容里包含这个词的条目"),
            }),
            &[],
        )
    }

    fn summarize(&self, _input: &Value) -> String {
        "查看长期记忆".into()
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        if !ctx.core.memory_enabled() {
            return Ok(ToolOutput::err("长期记忆功能现在是关闭的（设置 → Agent → 长期记忆）。"));
        }
        let kind_filter = arg_str(&input, "kind").and_then(|s| MemoryKind::parse(&s));
        let needle = arg_str(&input, "contains")
            .unwrap_or_default()
            .trim()
            .to_lowercase();

        let scopes = ctx.core.memory_scopes(ctx.topic.as_ref());
        // 先把要看的主题读进缓存（用户可能刚手动改过文件）
        if let Some(t) = ctx.topic.as_ref() {
            let mut chain = vec![t.clone()];
            chain.extend(ctx.core.workspace().ancestors(t));
            ctx.core.memory_write(|store| {
                for item in &chain {
                    store.load_topic(&item.slug(), &item.dir)?;
                }
                Ok(())
            })?;
        }
        let groups = ctx.core.memory_read(|store| {
            scopes
                .iter()
                .map(|(scope, name)| {
                    let label = match scope {
                        MemoryScope::Global => "全局（所有主题）".to_string(),
                        MemoryScope::Topic(slug) => {
                            if name.is_empty() {
                                format!("主题「{slug}」")
                            } else {
                                format!("主题「{name}」")
                            }
                        }
                    };
                    (label, store.list(scope))
                })
                .collect::<Vec<_>>()
        });

        let mut out = String::new();
        let mut total = 0usize;
        for (label, items) in groups {
            let hits: Vec<&Memory> = items
                .iter()
                .filter(|m| kind_filter.map(|k| m.kind == k).unwrap_or(true))
                .filter(|m| needle.is_empty() || m.content.to_lowercase().contains(&needle))
                .collect();
            if hits.is_empty() {
                continue;
            }
            out.push_str(&format!("## {label}\n"));
            for m in hits {
                total += 1;
                out.push_str(&format!(
                    "- [{}] {}｜{}｜用过 {} 次",
                    m.kind.label(),
                    m.content,
                    m.id,
                    m.use_count
                ));
                if m.pinned {
                    out.push_str("｜已钉住");
                }
                if !m.note.is_empty() {
                    out.push_str(&format!("｜备注：{}", m.note));
                }
                out.push('\n');
            }
        }
        if total == 0 {
            return Ok(ToolOutput::ok(
                "目前没有符合条件的长期记忆。遇到值得长期记住的事，用 memory_write 记下来。"
                    .to_string(),
            ));
        }
        out.push_str(&format!(
            "\n共 {total} 条。要改某条就 memory_write 带上它的 id；过时/记错的用 memory_forget 删掉。"
        ));
        Ok(ToolOutput::ok(out))
    }
}

pub struct MemoryForget;

#[async_trait]
impl Tool for MemoryForget {
    fn name(&self) -> &'static str {
        "memory_forget"
    }

    fn description(&self) -> &'static str {
        "删掉长期记忆里已经过时或记错的条目。\n\
         优先用 ids（先用 memory_list 拿到 id）；必要时可以给 content **原样**删除那一条，\
         但绝不模糊匹配——写得不完全一样就一条都删不掉。\n\
         用户说「忘掉这件事」「这个不对」时用它，不要只口头答应。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "ids": { "type": "array", "items": { "type": "string" }, "description": "要删的记忆 id（来自 memory_list）" },
                "content": str_prop("或者原样给出要删的那条内容（精确匹配）"),
                "scope": str_prop("global=全局；topic=某个主题（默认当前主题）"),
                "topic": str_prop("要删哪个主题里的记忆（只在 scope=topic 且不是当前主题时需要）"),
            }),
            &[],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        let n = arg_str_array(input, "ids").len();
        match arg_str(input, "content") {
            Some(c) if n == 0 => format!("忘掉：{}", c.chars().take(30).collect::<String>()),
            _ => format!("忘掉 {n} 条记忆"),
        }
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        if !ctx.core.memory_enabled() {
            return Ok(ToolOutput::err("长期记忆功能现在是关闭的（设置 → Agent → 长期记忆）。"));
        }
        let ids = arg_str_array(&input, "ids");
        let content = arg_str(&input, "content");
        let scope_word = arg_str(&input, "scope").unwrap_or_default();
        let to_global = matches!(scope_word.trim().to_ascii_lowercase().as_str(), "global" | "全局");

        let target = if to_global {
            None
        } else {
            match arg_str(&input, "topic").as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                Some(s) => Some(ctx.core.config_read().workspace().resolve(s)?),
                None => ctx.topic.clone(),
            }
        };
        let scope = match &target {
            Some(t) => MemoryScope::Topic(t.slug()),
            None => MemoryScope::Global,
        };
        let dir = target.as_ref().map(|t| t.dir.clone());
        let where_text = match &target {
            Some(t) => format!("主题「{}」", t.meta.name),
            None => "全局".to_string(),
        };

        let n = ctx.core.memory_write(|store| {
            store.forget(&scope, dir.as_deref(), &ids, content.as_deref())
        })?;
        if n > 0 {
            ctx.core.emit_memory_changed(&where_text);
        }
        Ok(ToolOutput::ok(if n > 0 {
            format!("已从{where_text}的记忆里删除 {n} 条。")
        } else {
            format!(
                "{where_text}的记忆里没有匹配的条目（id 或内容不完全一致）。\
                 先用 memory_list 看清楚再删。"
            )
        }))
    }
}

/// 这个工具名是不是记忆类工具。
///
/// 用途：总开关关掉时，把它们从**给模型的工具清单**里摘掉——
/// 关掉功能还留着工具，模型会一直尝试写然后每次都被拒，白烧 token。
pub fn is_memory_tool(name: &str) -> bool {
    matches!(name, "memory_write" | "memory_list" | "memory_forget")
}
