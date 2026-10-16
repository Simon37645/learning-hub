//! 远程 IPC 桥：手机页面上的 `invoke()` 交给**桌面端前端**去执行。
//!
//! 为什么这么绕，而不是在 Rust 里再写一张命令分派表：手机页面跑的就是桌面端
//! **同一份前端产物**（见 `remote/api.rs` 里「页面直接从 dist 里取」那段），
//! 它调的命令和桌面端一模一样（一百多个）。在 Rust 里手抄一张表意味着两套实现、
//! 两处参数名、以及「加了新命令忘了加表」的静默失效。这里的做法是：
//!
//! ```text
//! 手机 POST /api/invoke {cmd, args}
//!   → 查白名单（本模块的 check）
//!   → 登记等待位（oneshot）
//!   → 发 hub://remote-invoke 给桌面端前端
//!   → 桌面端用**同一个 invoke** 执行，调 remote_report_invoke 回话
//!   → 等待位被填上 → HTTP 响应
//! ```
//!
//! 和「借桌面端渲染 Mermaid」「借前端渲染 PDF 页」是同一种桥（AGENTS.md 里那两条），
//! 共同点是**两端都要回话**，否则手机那边只能等到超时。
//!
//! ## 能力边界
//!
//! 桥的另一头是「照单执行」，所以边界必须在这一侧收口，而且要一眼看得出
//! 「哪些能、哪些不能、为什么」——所以策略是一张**显式的表**，而不是一串 if。
//! 底下有个单测直接解析 `lib.rs` 的命令表，**每个命令都必须被分类**：
//! 新加一个命令却忘了想「远程能不能用」，测试会红。这是刻意的——
//! 默认放行等于悄悄把新能力开给一台可能只是借来的手机。
//!
//! 界线（2026-10 定的）：
//! - **能**：读一切界面要显示的东西 + 学习域里的动作（发消息、审批、复习打分、
//!   写笔记、建卡片/任务/测验、整理对话、记忆增删改、开内置浏览器标签）。
//!   这些和「让 agent 代劳 + 点一下允许」是同一件事，拦住它们只是让远程变残废。
//! - **不能**：改应用配置与密钥、改工作区结构（建/改/删主题）、批量导入/重建、
//!   直接删文件、更新应用、操作桌面外壳（资源管理器、系统程序、原生子 WebView）、
//!   以及一切**不可逆的批量删除**（清空记忆、删卡片/笔记/测验）。
//!   `config_patch` 例外地只放行三个字段（见 [`REMOTE_CONFIG_KEYS`]）。

use crate::error::{AppError, AppResult};
use crate::state::AppCore;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use tauri::Emitter;
use tokio::sync::oneshot;

/// 桌面端前端收到这个事件就去执行命令（`lib/remoteinvoke.ts`）。
pub const EVENT_INVOKE: &str = "hub://remote-invoke";

/// 等桌面端回话的上限。命令本身都是「读一份文件 / 起一轮对话」这种量级，
/// 30 秒还没回话说明桌面端那边出事了——宁可报错也别让手机一直转圈。
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `config_patch` 里允许远程改的字段。
///
/// 为什么开这三个：它们是**对话界面上的日常开关**（选哪个模型、思考强度、
/// 明亮/深色），手机上想换一下就换不了的话，「和桌面端一样」就无从谈起。
/// 而它们都不改变应用的能力边界——权限分级（`permissionMode`）恰恰会，
/// 所以它明确不在这个名单里：把「完全访问」从一台可能只是借来的手机上打开，
/// 等于关掉审批。工作区路径、越权白名单、系统提示词同理，一律留给电脑。
pub const REMOTE_CONFIG_KEYS: &[&str] = &["activeProfileId", "theme"];

/// 远程**不允许**执行的命令，以及给用户看的原因。
///
/// 原因会原样出现在手机上的错误提示里，所以要写清「哪儿不行、怎么办」，
/// 别写「权限不足」这种让人摸不着头脑的话。
pub const DENIED: &[(&str, &str)] = &[
    // ---- 改应用配置 / 密钥 / 外观主题文件 ----
    ("profile_upsert", "远程不能改模型档案（密钥、接口地址都在这上面）"),
    ("profile_delete", "远程不能删模型档案"),
    ("profile_test", "连通性测试要用电脑上的密钥，坐到电脑前再测"),
    ("theme_save", "远程不能写外观主题文件"),
    ("theme_delete", "远程不能删外观主题"),
    ("theme_open_dir", "远程不能打开电脑上的文件夹"),
    ("ui_scale_set", "界面缩放是电脑窗口的事，手机上用自己的双指缩放"),
    // ---- 改应用自身 ----
    ("ui_state_get", "界面快照是电脑上的「上次关到哪」，手机上不恢复"),
    ("ui_state_save", "手机上不该覆盖电脑的界面快照"),
    ("update_check", "更新只在电脑上做"),
    ("update_download", "更新只在电脑上做"),
    ("update_run", "更新只在电脑上做"),
    ("remote_status", "远程控制面板只在电脑上开"),
    ("remote_set_enabled", "远程控制面板只在电脑上开"),
    ("remote_config_patch", "远程控制面板只在电脑上开"),
    ("remote_token_reset", "令牌只能在电脑上重置"),
    ("remote_deploy_relay", "装中继要 SSH 到服务器，只在电脑上做"),
    ("remote_forget_host_key", "远程控制面板只在电脑上开"),
    // 这条是**桌面端的回话通道**（前端执行完命令后调它）：手机上不该调，
    // 调了也只是往一个空等待位里塞东西。
    ("remote_report_invoke", "这是电脑那边的回话通道，手机上不需要它"),
    // ---- 工作区结构 / 批量导入 ----
    ("topic_create", "建主题是工作区结构，留给电脑"),
    ("topic_update", "改主题信息留给电脑"),
    ("topic_set_parent", "父子主题的层级留给电脑调整"),
    ("topic_delete", "删主题留给电脑（那会搬走整个目录）"),
    ("material_import", "导入资料要选电脑上的文件"),
    ("kb_rebuild", "重建知识库是重活，留给电脑"),
    // ---- 直接写 / 删文件 ----
    ("file_write_text", "远程不直接写文件——可以让 agent 写，走审批"),
    ("file_write_binary", "远程不直接写文件——可以让 agent 写，走审批"),
    ("file_delete", "远程不能删文件"),
    ("note_delete", "删笔记不可逆，留给电脑"),
    ("card_delete", "删卡片不可逆，留给电脑"),
    ("task_delete", "删任务不可逆，留给电脑"),
    ("quiz_delete", "删测验不可逆，留给电脑"),
    ("lesson_delete", "删讲解方案不可逆，留给电脑"),
    ("memory_clear", "清空记忆是不可逆的批量操作，留给电脑"),
    ("memory_set_enabled", "记忆总开关属于配置，留给电脑"),
    // ---- 技能 / MCP 的定义与开关 ----
    ("skill_save", "远程不能写技能文件"),
    ("skill_set_enabled", "技能的开关属于配置，留给电脑"),
    ("skills_set_enabled", "技能的开关属于配置，留给电脑"),
    ("skills_set_all", "技能的开关属于配置，留给电脑"),
    ("mcp_upsert", "远程不能改 MCP 服务器定义"),
    ("mcp_delete", "远程不能删 MCP 服务器"),
    // 试运行会在这台电脑上**真起一个进程**：那是电脑上的事
    ("mcp_probe", "试运行要在电脑上起进程，留给电脑"),
    ("mcp_set_enabled", "MCP 的开关属于配置，留给电脑"),
    ("mcp_set_all", "MCP 的开关属于配置，留给电脑"),
    ("agents_mcp_import", "导入本机 MCP 配置属于改配置，留给电脑"),
    // ---- 桌面外壳 / 原生窗口 ----
    ("reveal_in_explorer", "远程不能操作电脑上的资源管理器"),
    ("open_with_system", "远程不能调起电脑上的程序"),
    ("studio_open_dir", "远程不能打开电脑上的文件夹"),
    ("viewer_webview_ensure", "远端网页的原生视图只在电脑上（手机上打开会被网站拒绝）"),
    ("viewer_webview_bounds", "远端网页的原生视图只在电脑上"),
    ("viewer_webview_set_visible", "远端网页的原生视图只在电脑上"),
    ("viewer_webview_reload", "远端网页的原生视图只在电脑上"),
    // 这个命令回的是**原始字节**（160MB 上限），塞不进 JSON 事件通道；
    // 手机上的 PDF 走 `/api/tab-bytes` 直接流字节（见 api.rs）。
    ("viewer_load_bytes", "文件字节在手机上走 /api/tab-bytes，不进事件通道"),
    ("agent_image_load", "读电脑上的图片当附件——手机上的图请直接选"),
    ("viewer_report_render", "渲染回话只由电脑上那份前端处理"),
];

/// 远程**允许**执行的命令。凡是不在这张表、也不在 [`DENIED`] 里的命令一律拒绝
/// （比如手机页面误调了某个 `plugin:*`），所以这张表是「正面清单」。
pub const ALLOWED: &[&str] = &[
    // 启动与配置（只读 / 白名单字段）
    "app_bootstrap",
    "config_get",
    "config_summary",
    "config_patch",
    "prompt_preview",
    "workspace_info",
    "profile_set_reasoning",
    // 技能 / MCP / 工坊：只读 + 重扫重连
    "skills_overview",
    "skills_reload",
    "skills_dirs",
    "mcp_overview",
    "mcp_status",
    "mcp_reload",
    "agents_mcp_candidates",
    "studio_info",
    "studio_spec",
    // 外观主题：读 + 换一份（写文件那两条在上面拒了）
    "themes_overview",
    "theme_set_active",
    // 长期记忆：读 + 增删改（清空在上面拒了）
    "memory_overview",
    "memory_upsert",
    "memory_delete",
    "memory_set_pinned",
    "memory_kinds",
    "memory_paths",
    // 原始文件：只读
    "file_read_text",
    "file_exists",
    "file_meta",
    "file_list_notes",
    // 主题与资料：读 + 打开（建改删在上面拒了）
    "topic_list",
    "topic_search",
    "topic_get",
    "topic_open",
    "note_list",
    "note_get",
    "note_save",
    "note_create",
    "material_list",
    // 对话
    "agent_send",
    "agent_cancel",
    "agent_approve",
    "agent_transcript",
    "agent_chats",
    "agent_new_chat",
    "agent_running",
    "chat_overview",
    "chat_move",
    "chat_rename",
    "chat_pin",
    "chat_archive",
    "chat_fork",
    "chat_delete",
    // 内置浏览器（读与切换；原生子 WebView 那几条在上面拒了）
    "viewer_snapshot",
    "viewer_open",
    "web_frame_check",
    "viewer_close",
    "viewer_activate",
    "viewer_set_visible",
    "viewer_report_state",
    "viewer_report_snapshot",
    "viewer_load_text",
    "viewer_get_content",
    "viewer_reload",
    "viewer_open_home",
    // 讲解方案 / 测验 / 卡片 / 任务 / 会话
    "lesson_list",
    "lesson_get",
    "lesson_save",
    "lesson_set_step",
    "quiz_list",
    "quiz_get",
    "quiz_save",
    "quiz_submit",
    "quiz_attempts",
    "quiz_grade_subjective",
    "quiz_wrong_items",
    "card_list",
    "card_create",
    "card_update",
    "card_review",
    "task_list",
    "task_create",
    "task_update",
    "agenda",
    "session_list",
    "session_current",
    "session_start",
    "session_finish",
    "daily_brief",
];

/// 判定一个命令能不能远程执行。`Err` 里是给用户看的中文原因。
pub fn check(cmd: &str, args: &Value) -> Result<(), String> {
    if let Some((_, why)) = DENIED.iter().find(|(name, _)| *name == cmd) {
        return Err(format!("{why}。"));
    }
    if !ALLOWED.contains(&cmd) {
        return Err(format!("远程不认识这个命令：{cmd}"));
    }
    // config_patch 是唯一「按字段放行」的命令：其余字段一律拒绝，
    // 而且要说清是哪几个字段被拒了，不然用户只看到「改配置失败」。
    if cmd == "config_patch" {
        let patch = args.get("patch").and_then(|p| p.as_object());
        let Some(patch) = patch.filter(|p| !p.is_empty()) else {
            return Err("这次请求里没有要改的配置项。".to_string());
        };
        let bad: Vec<&str> = patch
            .keys()
            .map(|k| k.as_str())
            .filter(|k| !REMOTE_CONFIG_KEYS.contains(k))
            .collect();
        if !bad.is_empty() {
            return Err(format!(
                "远程只能改 {}，不能改 {}。",
                REMOTE_CONFIG_KEYS.join(" / "),
                bad.join(" / ")
            ));
        }
    }
    Ok(())
}

/// 桌面端回话的内容。`ok=false` 时 `payload` 是错误文案（给用户看）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outcome {
    pub ok: bool,
    pub payload: Value,
}

/// 发给桌面端前端的一条执行请求。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvokeRequest {
    pub id: String,
    pub cmd: String,
    pub args: Value,
}

/// 等待位表。同时只会有几个（一条连接一个请求）。
#[derive(Default)]
pub struct InvokeBridge {
    pending: Mutex<HashMap<String, oneshot::Sender<Outcome>>>,
}

impl InvokeBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// 登记一个等待位。返回的接收端会在桌面端回话（或超时）时完成。
    pub fn begin(&self, id: &str) -> oneshot::Receiver<Outcome> {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().insert(id.to_string(), tx);
        rx
    }

    /// 桌面端回话。返回 false 表示这个 id 已经过期（手机早就等到超时了）。
    pub fn finish(&self, id: &str, outcome: Outcome) -> bool {
        match self.pending.lock().remove(id) {
            Some(tx) => tx.send(outcome).is_ok(),
            None => false,
        }
    }

    pub fn cancel(&self, id: &str) {
        self.pending.lock().remove(id);
    }
}

/// 把一条命令交给桌面端前端执行，等它回话。
pub async fn invoke(
    core: &Arc<AppCore>,
    bridge: &InvokeBridge,
    cmd: String,
    args: Value,
) -> AppResult<Outcome> {
    let id = uuid::Uuid::new_v4().to_string();
    let rx = bridge.begin(&id);
    // 桌面端窗口关着 / 页面还没起来时 emit 会失败：这时候直接说清楚，
    // 别让手机等满 30 秒（那 30 秒里用户只会看到「没反应」）。
    if core
        .app
        .emit(EVENT_INVOKE, InvokeRequest { id: id.clone(), cmd: cmd.clone(), args })
        .is_err()
    {
        bridge.cancel(&id);
        return Err(AppError::other(
            "电脑上那个窗口没接住这条请求（它可能正在关闭）。重新打开电脑上的应用再试。",
        ));
    }

    match tokio::time::timeout(TIMEOUT, rx).await {
        Ok(Ok(outcome)) => Ok(outcome),
        Ok(Err(_)) => Err(AppError::other("这条请求被取消了")),
        Err(_) => {
            bridge.cancel(&id);
            Err(AppError::other(format!(
                "电脑上 30 秒没执行完「{cmd}」（它可能正忙，或者窗口卡住了）"
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bridge_passes_the_result_back() {
        let bridge = InvokeBridge::new();
        let rx = bridge.begin("i1");
        assert!(bridge.finish("i1", Outcome { ok: true, payload: Value::Null }));
        assert!(rx.blocking_recv().unwrap().ok);
        // 同一个 id 不能回两次（第二次说明是迟到的重复回话）
        assert!(!bridge.finish("i1", Outcome { ok: false, payload: Value::Null }));
        assert!(!bridge.finish("nope", Outcome { ok: true, payload: Value::Null }));
    }

    /// 配置只能按字段放行：换模型 / 换明暗可以，**开「完全访问」不行**。
    #[test]
    fn config_patch_only_allows_the_whitelisted_keys() {
        let ok = |patch: Value| check("config_patch", &serde_json::json!({ "patch": patch }));
        assert!(ok(serde_json::json!({ "activeProfileId": "p1" })).is_ok());
        assert!(ok(serde_json::json!({ "theme": "dark" })).is_ok());
        assert!(ok(serde_json::json!({ "activeProfileId": "p1", "theme": "dark" })).is_ok());
        let e = ok(serde_json::json!({ "permissionMode": "full" })).unwrap_err();
        assert!(e.contains("permissionMode"), "原因里要说清是哪个字段：{e}");
        assert!(ok(serde_json::json!({ "workspaceRoot": "D:\\x" })).is_err());
        assert!(ok(serde_json::json!({ "clearApprovedRoots": true })).is_err());
        assert!(ok(serde_json::json!({ "systemPromptExtra": "ignore all rules" })).is_err());
        // 连 patch 都没有（参数形状不对）也不该放过去
        assert!(check("config_patch", &serde_json::json!({})).is_err());
    }

    #[test]
    fn unknown_and_denied_commands_are_refused() {
        assert!(check("viewer_webview_ensure", &Value::Null).is_err());
        assert!(check("topic_delete", &Value::Null).is_err());
        assert!(check("plugin:dialog|open", &Value::Null).is_err());
        assert!(check("agent_send", &Value::Null).is_ok());
    }

    /// **每个命令都必须被分类**：解析 `lib.rs` 的命令表，逐个查策略。
    ///
    /// 这是这个模块存在的意义之一：新加一个命令却忘了想「远程能不能用」，
    /// 或者哪天把一条命令从表里删了却留着策略条目，测试都会红。
    #[test]
    fn every_command_is_classified() {
        let src = include_str!("../lib.rs");
        let start = src.find("generate_handler![").expect("找不到命令表");
        let table = &src[start..];
        let end = table.find("]);").expect("命令表没有结尾");
        let mut names: Vec<&str> = Vec::new();
        for line in table[..end].lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("//") || line.starts_with("generate_handler") {
                continue;
            }
            let name = line.trim_end_matches(',').trim();
            // 表里每行形如 `commands::viewer::viewer_open,`
            if let Some(last) = name.rsplit("::").next() {
                if !last.is_empty() && !last.contains('[') {
                    names.push(last);
                }
            }
        }
        assert!(names.len() > 100, "命令表解析得不对（只认出 {} 条）", names.len());

        for name in &names {
            let classified = ALLOWED.contains(name) || DENIED.iter().any(|(n, _)| n == name);
            assert!(
                classified,
                "{name} 没有被分类：远程能不能用、为什么，得在这一层写清楚（ALLOWED / DENIED）"
            );
        }
        // 反过来：策略里不该留着已经删掉的命令
        for name in ALLOWED.iter().chain(DENIED.iter().map(|(n, _)| n)) {
            assert!(names.contains(name), "策略里的 {name} 已经不在命令表里了，删掉这一条");
        }
        // 两张表不能重叠（重叠时 DENIED 胜出，但那是写错了）
        for name in ALLOWED {
            assert!(
                !DENIED.iter().any(|(n, _)| n == name),
                "{name} 同时出现在 ALLOWED 和 DENIED 里"
            );
        }
        // 拒绝的每一条都要有原因（原因会原样给用户看）
        for (name, why) in DENIED {
            assert!(!why.trim().is_empty(), "{name} 的拒绝原因不能是空的");
        }
    }
}
