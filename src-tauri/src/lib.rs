//! 学习中枢 —— 库入口。
//!
//! 分层：
//! ```text
//! commands/   Tauri IPC 层（前端可见的接口）
//! agent/      对话循环、模型接入、工具
//! viewer/     内置浏览器状态机与网页正文提取
//! domain/     主题/笔记/卡片/任务/会话等纯数据模型
//! store.rs    文件读写（磁盘即数据库）
//! config.rs   配置
//! net.rs      出网
//! paths.rs    路径安全
//! ```

pub mod agent;
pub mod anki;
pub mod commands;
pub mod config;
pub mod domain;
pub mod error;
pub mod kb;
pub mod mcp;
pub mod net;
pub mod paths;
pub mod skills;
pub mod state;
pub mod store;
pub mod viewer;

use crate::config::AppConfig;
use crate::state::{AppCore, AppState};
use std::time::Duration;
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .setup(|app| {
            let handle = app.handle().clone();

            // 配置放在系统标准的应用配置目录
            let config_path = handle
                .path()
                .app_config_dir()
                .map(|d| d.join("config.json"))
                .unwrap_or_else(|_| std::path::PathBuf::from("config.json"));

            // 默认工作区：文档目录下的「学习中枢」
            let default_ws = handle
                .path()
                .document_dir()
                .map(|d| d.join("学习中枢"))
                .unwrap_or_else(|_| std::path::PathBuf::from("学习中枢"));

            let cfg = AppConfig::load(&config_path, default_ws)?;
            let http = net::client(600).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            let core = AppCore::new(handle, config_path, cfg, http);

            // 笔记里的本地图片通过 asset:// 显示。范围收窄到工作区，
            // 让「只能碰工作区」的沙箱语义在界面层也成立。
            {
                let ws = core.config_read().workspace_root.clone();
                if let Err(e) = core.allow_asset_dir(&ws) {
                    eprintln!("[启动] 放开静态资源目录失败：{e}");
                }
            }

            // 技能：扫一遍目录（几十毫秒），之后可以在设置里重扫
            let skill_count = core.reload_skills();

            // MCP：连接是异步且可能要等子进程，放到后台，不阻塞开窗
            {
                let core_bg = core.clone();
                tauri::async_runtime::spawn(async move {
                    let status = core_bg.mcp_reload().await;
                    let ok = status.iter().filter(|s| s.connected).count();
                    if !status.is_empty() {
                        eprintln!("[mcp] {} 个服务器，{} 个连接成功", status.len(), ok);
                    }
                    let _ = skill_count;
                });
            }

            // 工作区骨架先建好，用户第一眼就能看到目录结构
            if let Err(e) = core.workspace().ensure() {
                eprintln!("[启动] 工作区初始化失败：{e}");
            }

            app.manage(AppState(core));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // --- 技能与 MCP ---
            commands::extend::skills_overview,
            commands::extend::skills_reload,
            commands::extend::skills_dirs,
            commands::extend::skill_save,
            commands::extend::skill_set_enabled,
            commands::extend::skills_set_enabled,
            commands::extend::mcp_overview,
            commands::extend::mcp_status,
            commands::extend::mcp_reload,
            commands::extend::mcp_upsert,
            commands::extend::mcp_delete,
            commands::extend::mcp_set_enabled,
            // --- 应用 ---
            commands::app::app_bootstrap,
            commands::app::config_get,
            commands::app::config_patch,
            commands::app::profile_upsert,
            commands::app::profile_delete,
            commands::app::profile_test,
            commands::app::prompt_preview,
            commands::app::workspace_info,
            commands::app::reveal_in_explorer,
            commands::app::open_with_system,
            commands::app::config_summary,
            // --- 原始文件读写（内置编辑器用）---
            commands::file::file_read_text,
            commands::file::file_write_text,
            commands::file::file_write_binary,
            commands::file::file_exists,
            commands::file::file_delete,
            commands::file::file_meta,
            commands::file::file_list_notes,
            // --- 主题 / 笔记 / 资料 ---
            commands::topic::topic_list,
            commands::topic::topic_search,
            commands::topic::topic_create,
            commands::topic::topic_get,
            commands::topic::topic_open,
            commands::topic::topic_update,
            commands::topic::topic_delete,
            commands::topic::note_list,
            commands::topic::note_get,
            commands::topic::note_save,
            commands::topic::note_create,
            commands::topic::note_delete,
            commands::topic::material_list,
            commands::topic::material_import,
            // --- 对话 ---
            commands::agent::agent_send,
            commands::agent::agent_cancel,
            commands::agent::agent_approve,
            commands::agent::agent_transcript,
            commands::agent::agent_chats,
            commands::agent::agent_new_chat,
            commands::agent::agent_running,
            // --- 内置浏览器 ---
            commands::viewer::viewer_snapshot,
            commands::viewer::viewer_open,
            commands::viewer::viewer_close,
            commands::viewer::viewer_activate,
            commands::viewer::viewer_set_visible,
            commands::viewer::viewer_report_state,
            commands::viewer::viewer_report_snapshot,
            commands::viewer::viewer_load_text,
            commands::viewer::viewer_load_bytes,
            commands::viewer::viewer_get_content,
            commands::viewer::viewer_reload,
            commands::viewer::viewer_open_home,
            // --- 讲解方案 ---
            commands::lesson::lesson_list,
            commands::lesson::lesson_get,
            commands::lesson::lesson_save,
            commands::lesson::lesson_set_step,
            commands::lesson::lesson_delete,
            // --- 测验 ---
            commands::quiz::quiz_list,
            commands::quiz::quiz_get,
            commands::quiz::quiz_save,
            commands::quiz::quiz_delete,
            commands::quiz::quiz_submit,
            commands::quiz::quiz_attempts,
            commands::quiz::quiz_grade_subjective,
            commands::quiz::quiz_wrong_items,
            // --- 学习资产 ---
            commands::study::card_list,
            commands::study::card_create,
            commands::study::card_update,
            commands::study::card_delete,
            commands::study::card_review,
            commands::study::card_export_anki,
            commands::study::anki_status,
            commands::study::anki_sync,
            commands::study::anki_ping,
            commands::study::task_list,
            commands::study::task_create,
            commands::study::task_update,
            commands::study::task_delete,
            commands::study::agenda,
            commands::study::session_list,
            commands::study::session_current,
            commands::study::session_start,
            commands::study::session_finish,
            commands::study::daily_brief,
        ])
        .run(tauri::generate_context!())
        .expect("学习中枢启动失败");
}

/// 给测试用的常量：默认 HTTP 超时。
pub const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(600);
