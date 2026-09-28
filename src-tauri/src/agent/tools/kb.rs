//! 知识库工具：让 agent 检索用户的讲义，并标出来源。
//!
//! 与 fs_search 的分工：fs_search 是「关键词在哪个文件第几行」，
//! kb_search 是「这个问题的相关段落在哪，出自哪份资料的哪一页」——
//! 后者带 PDF 页码，能直接变成引用。

use crate::agent::registry::{
    arg_bool, arg_str, arg_str_req, arg_u32, bool_prop, num_prop, object_schema, str_prop, Tool,
    ToolCtx, ToolOutput,
};
use crate::error::AppResult;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct KbBuild;

#[async_trait]
impl Tool for KbBuild {
    fn name(&self) -> &'static str {
        "kb_build"
    }

    fn description(&self) -> &'static str {
        "刷新知识库索引：把主题里的讲义（kb/、materials/）和笔记（notes/）抽成可检索的文本。
         用户刚放进新的讲义、或改了笔记之后调用它。
         对扫描件（图片型 PDF）会标记「读不出文字」，这时应当请用户先做 OCR 或换成文字版。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "topic": str_prop("主题名，默认当前主题"),
                "force": bool_prop("是否强制重建（默认只抽取有变化的文件）"),
            }),
            &[],
        )
    }

    fn risk(&self) -> crate::agent::event::Risk {
        crate::agent::event::Risk::Write
    }

    fn summarize(&self, _input: &Value) -> String {
        "刷新知识库索引".into()
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic_or(arg_str(&input, "topic").as_deref())?;
        let force = arg_bool(&input, "force").unwrap_or(false);
        let index = crate::kb::build(&topic.dir, force).await?;
        let mut out = format!(
            "索引完成：{} 份文件、{} 个片段、共约 {} 字。

{}",
            index.files.len(),
            index.chunks.len(),
            index.total_chars(),
            crate::kb::digest(&index, 40, 6)
        );
        let unreadable: Vec<&str> = index
            .files
            .iter()
            .filter(|f| f.chars == 0 && f.error.is_some())
            .map(|f| f.path.as_str())
            .collect();
        if !unreadable.is_empty() {
            out.push_str(&format!(
                "

有 {} 份资料读不出文字（可能是扫描件）：{}。可以考虑请用户换成文字版。",
                unreadable.len(),
                unreadable.join("、")
            ));
        }
        Ok(ToolOutput::ok(out))
    }
}

pub struct KbSearch;

#[async_trait]
impl Tool for KbSearch {
    fn name(&self) -> &'static str {
        "kb_search"
    }

    fn description(&self) -> &'static str {
        "在知识库里检索相关段落，返回带**来源标注**的片段（文件名 + PDF 页码）。
         讲解知识点、回答用户提问之前先检索一遍，讲的时候就能标出处。
         引用格式请写成：【来源：文件相对路径 第 N 页】"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "query": str_prop("检索词，可以是多个词，空格分隔"),
                "topic": str_prop("主题名，默认当前主题"),
                "limit": num_prop("最多返回几段，默认 6"),
                "auto_build": bool_prop("索引为空时自动建一次，默认 true"),
            }),
            &["query"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("知识库检索「{}」", arg_str(input, "query").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic_or(arg_str(&input, "topic").as_deref())?;
        let query = arg_str_req(&input, "query")?;
        let limit = arg_u32(&input, "limit").unwrap_or(6).min(30) as usize;

        let mut index = crate::kb::load_index(&topic.dir);
        if index.chunks.is_empty() && arg_bool(&input, "auto_build").unwrap_or(true) {
            index = crate::kb::build(&topic.dir, false).await?;
        }
        if index.chunks.is_empty() {
            return Ok(ToolOutput::ok(
                "知识库还是空的。这个主题的 materials/、kb/、notes/ 里还没有可读文本。".to_string(),
            ));
        }

        let hits = crate::kb::search(&index, &query, limit);
        if hits.is_empty() {
            return Ok(ToolOutput::ok(format!(
                "知识库里没有找到「{query}」（索引了 {} 份文件）。可以换个说法，或用 fs_search 全文找。",
                index.files.len()
            )));
        }

        let mut out = format!("知识库命中 {} 段：

", hits.len());
        for (i, h) in hits.iter().enumerate() {
            out.push_str(&format!("{}. 【来源：{}】
   {}

", i + 1, h.label, h.context));
        }
        out.push_str("引用时请照抄上面的【来源：…】标注，用户点它就能跳到原文。");
        Ok(ToolOutput::ok(out))
    }
}
