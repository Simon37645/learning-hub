//! 内置工具集装配。
//!
//! 按用途分四组：
//! - `fs`     文件读写与全文检索
//! - `study`  主题 / 笔记 / 卡片 / 计划 / 学习会话
//! - `viewer` 内置浏览器的操作
//! - `web`    联网抓取与搜索
//!
//! 想加工具：在对应文件里 `impl Tool`，然后在 [`registry`] 里 `register`。

pub mod fs;
pub mod kb;
pub mod lesson;
pub mod mcp;
pub mod memory;
pub mod mindmap;
pub mod quiz;
pub mod skills;
pub mod study;
pub mod viewer;
pub mod web;

use crate::agent::registry::ToolRegistry;
use std::sync::Arc;

/// 构造带全部内置工具的注册表。
pub fn registry() -> ToolRegistry {
    let mut r = ToolRegistry::new();

    // --- 主题与学习闭环 ---
    r.register(Arc::new(study::TopicCreate))
        .register(Arc::new(study::TopicList))
        .register(Arc::new(study::TopicInfo))
        .register(Arc::new(study::TopicSetStage))
        .register(Arc::new(study::NoteCreate))
        .register(Arc::new(study::NoteList))
        .register(Arc::new(study::CardCreate))
        .register(Arc::new(study::CardList))
        .register(Arc::new(study::CardReview))
        .register(Arc::new(study::TaskCreate))
        .register(Arc::new(study::TaskList))
        .register(Arc::new(study::TaskUpdate))
        .register(Arc::new(study::SessionStart))
        .register(Arc::new(study::SessionFinish))
        .register(Arc::new(study::SessionList));

    // --- 文件 ---
    r.register(Arc::new(fs::FsList))
        .register(Arc::new(fs::FsRead))
        .register(Arc::new(fs::FsWrite))
        .register(Arc::new(fs::FsMkdir))
        .register(Arc::new(fs::FsMove))
        .register(Arc::new(fs::FsDelete))
        .register(Arc::new(fs::FsSearch))
        .register(Arc::new(fs::MaterialImport));

    // --- 测验 ---
    r.register(Arc::new(quiz::QuizCreate))
        .register(Arc::new(quiz::QuizList))
        .register(Arc::new(quiz::QuizGet))
        .register(Arc::new(quiz::QuizGrade));

    // --- 表达与图示 ---
    r.register(Arc::new(mindmap::MindmapCreate));

    // --- 讲解方案 ---
    r.register(Arc::new(lesson::LessonPlanTool))
        .register(Arc::new(lesson::LessonStepTool));

    // --- 知识库 ---
    r.register(Arc::new(kb::KbBuild)).register(Arc::new(kb::KbSearch));

    // --- 内置浏览器 ---
    r.register(Arc::new(viewer::ViewerOpen))
        .register(Arc::new(viewer::ViewerList))
        .register(Arc::new(viewer::ViewerRead))
        .register(Arc::new(viewer::ViewerGoto))
        .register(Arc::new(viewer::ViewerSearch))
        .register(Arc::new(viewer::ViewerActivate))
        .register(Arc::new(viewer::ViewerClose));

    // --- 技能与 MCP ---
    r.register(Arc::new(skills::SkillList))
        .register(Arc::new(skills::SkillRead))
        .register(Arc::new(skills::McpStatus));

    // --- 长期记忆（总开关关闭时会在 run_turn 里被摘掉）---
    r.register(Arc::new(memory::MemoryWrite))
        .register(Arc::new(memory::MemoryList))
        .register(Arc::new(memory::MemoryForget));

    // --- 联网 ---
    r.register(Arc::new(web::WebFetch)).register(Arc::new(web::WebSearch));

    r
}
