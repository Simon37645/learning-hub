//! 对话相关命令：发消息、中止、审批、读历史。

use crate::agent::message::ChatMessage;
use crate::agent::registry::AgentMode;
use crate::agent::TurnRequest;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use serde::Serialize;
use tauri::State;

/// 前端送来的 `mode` 参数：认不出来一律当学习模式（老前端不传这个字段）。
fn mode_of(raw: Option<&str>) -> AgentMode {
    AgentMode::parse(raw.unwrap_or(""))
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub turn_id: String,
    pub chat_id: String,
    /// 这一轮在哪个模式里跑的（前端据此确认自己的清单/视图没跑偏）
    pub mode: AgentMode,
    /// 落库后的用户消息（带正式 id，前端据此渲染）
    pub message: ChatMessage,
}

#[tauri::command]
pub async fn agent_send(state: State<'_, AppState>, req: TurnRequest) -> AppResult<SendResult> {
    let core = state.0.clone();
    let chat_id = req.chat_id.clone();
    let mode = req.mode;
    let (turn_id, message) = core.agent.start_turn(core.clone(), req)?;
    Ok(SendResult { turn_id, chat_id, mode, message })
}

#[tauri::command]
pub async fn agent_cancel(state: State<'_, AppState>, turn_id: String) -> AppResult<bool> {
    Ok(state.0.agent.cancel(&turn_id))
}

/// 读一张本机图片，转成可以随消息上传的载荷（用户在输入框里「选图片」时用）。
///
/// 这里**不做沙箱申请**：路径来自系统的文件选择框，是用户主动挑的那张图，
/// 和学习模式里「模型自己拼出来的路径」不是一回事。文件本身只读一遍、不落盘，
/// 真正落盘的是发送时统一走的那条路（`start_turn` → attachments）。
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadedImagePayload {
    /// 原文件名（只用于显示）
    pub name: String,
    pub media_type: String,
    /// 裸 base64
    pub data: String,
}

#[tauri::command]
pub async fn agent_image_load(path: String) -> AppResult<LoadedImagePayload> {
    let p = std::path::PathBuf::from(crate::paths::expand_home(path.trim()));
    if !p.is_file() {
        // 注意别在消息里重复「找不到」——AppError::NotFound 的 Display 已经带了前缀
        return Err(AppError::NotFound(format!("文件不存在：{}", p.display())));
    }
    let size = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
    if size > crate::agent::attachment::MAX_IMAGE_BYTES {
        return Err(AppError::invalid(format!(
            "这张图太大了（{}），上限 {}",
            crate::paths::human_size(size),
            crate::paths::human_size(crate::agent::attachment::MAX_IMAGE_BYTES)
        )));
    }
    let bytes = std::fs::read(&p).map_err(|e| crate::store::io_err(&p, e))?;
    let media_type = crate::agent::attachment::detect_media_type(&bytes).ok_or_else(|| {
        AppError::invalid("只支持 PNG / JPEG / GIF / WebP 四种图片")
    })?;
    Ok(LoadedImagePayload {
        name: p
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "图片".into()),
        media_type: media_type.to_string(),
        data: crate::agent::attachment::b64_encode(&bytes),
    })
}

/// 回应工具审批弹窗。`always=true` 表示本次运行内一直允许这个工具。
#[tauri::command]
pub async fn agent_approve(
    state: State<'_, AppState>,
    request_id: String,
    allow: bool,
    always: Option<bool>,
) -> AppResult<bool> {
    Ok(state
        .0
        .agent
        .respond_approval(&request_id, allow, always.unwrap_or(false)))
}

#[tauri::command]
pub async fn agent_transcript(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
    chat_id: String,
) -> AppResult<Vec<ChatMessage>> {
    crate::agent::read_transcript(&state.0, topic_slug.as_deref(), &chat_id)
}

#[tauri::command]
pub async fn agent_chats(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
) -> AppResult<Vec<String>> {
    crate::agent::list_transcripts(&state.0, topic_slug.as_deref())
}

/// 生成一个新对话 id（不落盘，等第一条消息写入时才产生文件）。
#[tauri::command]
pub async fn agent_new_chat() -> AppResult<String> {
    Ok(uuid::Uuid::new_v4().to_string())
}

/// 侧栏「对话」列表里的一项。
///
/// 排序在后端算好（置顶 → 最近使用 → 归档沉底），前端照着画就行。
pub use crate::agent::chats::ChatOverviewItem;

/// 读一个主题的对话清单并排好序。`include_archived=false` 时滤掉归档的。
///
/// 上限（60）放在**排序之后**：不然置顶的那条可能因为文件太旧被截掉，
/// 用户会看到「置顶了但没置顶」。
///
/// `mode` 决定这份清单属于谁：工作区级的对话（没有主题的那些）里，
/// 首页的日常问答与工坊里的对话放在同一个目录，靠侧车文件里的 `mode` 分开。
fn collect_overview(
    core: &std::sync::Arc<crate::state::AppCore>,
    slug: Option<&str>,
    mode: AgentMode,
    include_archived: bool,
) -> AppResult<Vec<ChatOverviewItem>> {
    // 实现在 agent::chats::collect（移动端远程接口也走同一份，排序口径只有一处）
    crate::agent::chats::collect(core, slug, mode, include_archived)
}

/// 某个主题（或工作区级）下的历史对话清单（侧栏用）。
///
/// 标题优先取侧车文件里的自定义名字，没有才现算「第一句用户消息」。
/// `include_archived` 为 false 时把归档的滤掉——侧栏日常只显示没归档的。
#[tauri::command]
pub async fn chat_overview(
    state: State<'_, AppState>,
    topic_slug: Option<String>,
    mode: Option<String>,
    include_archived: Option<bool>,
) -> AppResult<Vec<ChatOverviewItem>> {
    let core = state.0.clone();
    collect_overview(
        &core,
        topic_slug.as_deref(),
        mode_of(mode.as_deref()),
        include_archived.unwrap_or(false),
    )
}

/// 改完元数据后统一回一次新清单，前端直接拿它替换本地状态。
fn overview_after_change(
    core: &std::sync::Arc<crate::state::AppCore>,
    slug: &Option<String>,
    mode: AgentMode,
) -> AppResult<Vec<ChatOverviewItem>> {
    // 主题统计（对话条数）也跟着变，让侧栏的计数与折叠箭头一致
    core.emit_topics(crate::agent::event::TopicsEvent::Refresh);
    collect_overview(core, slug.as_deref(), mode, true)
}

/// 改对话名字。空标题＝恢复成「第一句话」的自动标题。
#[tauri::command]
pub async fn chat_rename(
    state: State<'_, AppState>,
    chat_id: String,
    title: String,
    topic_slug: Option<String>,
    mode: Option<String>,
) -> AppResult<Vec<ChatOverviewItem>> {
    let core = state.0.clone();
    let mode = mode_of(mode.as_deref());
    let dir = crate::agent::chats_dir_for(&core, topic_slug.as_deref());
    let mut meta = crate::agent::chats::load(&dir, &chat_id);
    meta.rename(&title);
    crate::agent::chats::save(&dir, &chat_id, &mut meta)?;
    overview_after_change(&core, &topic_slug, mode)
}

/// 置顶 / 取消置顶。
#[tauri::command]
pub async fn chat_pin(
    state: State<'_, AppState>,
    chat_id: String,
    pinned: bool,
    topic_slug: Option<String>,
    mode: Option<String>,
) -> AppResult<Vec<ChatOverviewItem>> {
    let core = state.0.clone();
    let mode = mode_of(mode.as_deref());
    let dir = crate::agent::chats_dir_for(&core, topic_slug.as_deref());
    let mut meta = crate::agent::chats::load(&dir, &chat_id);
    meta.pinned = pinned;
    if pinned {
        // 置顶与归档是互斥的：从归档里捞出来置顶，意思很明确
        meta.archived = false;
    }
    crate::agent::chats::save(&dir, &chat_id, &mut meta)?;
    overview_after_change(&core, &topic_slug, mode)
}

/// 归档 / 取消归档。归档的对话默认收进侧栏的「已归档」分组。
#[tauri::command]
pub async fn chat_archive(
    state: State<'_, AppState>,
    chat_id: String,
    archived: bool,
    topic_slug: Option<String>,
    mode: Option<String>,
) -> AppResult<Vec<ChatOverviewItem>> {
    let core = state.0.clone();
    let mode = mode_of(mode.as_deref());
    let dir = crate::agent::chats_dir_for(&core, topic_slug.as_deref());
    let mut meta = crate::agent::chats::load(&dir, &chat_id);
    meta.archived = archived;
    if archived {
        // 归档就是「先收起来」：留着置顶标记会让它在置顶区又冒出来
        meta.pinned = false;
    }
    crate::agent::chats::save(&dir, &chat_id, &mut meta)?;
    overview_after_change(&core, &topic_slug, mode)
}

/// 分叉：把这条对话在「最后一个完整回合」处截断，复制成一条新对话。
///
/// 用途是「从这儿换个讲法重来」——原对话保持不动，新对话继承到这里为止的上下文。
/// 截断点定在**最后一条 assistant 消息（含）连同它后面成对的 tool_result**：
/// - 末尾若挂着「用户刚提问、模型还没答」，复制过去会让新对话一开头就欠一个回答；
/// - 但 assistant 后面成对落库的 tool 行**不能切**（典型：中止一轮后分叉，那几行
///   「已被用户中止」的结果就挂在那条 assistant 后面）——切了的话新对话以
///   「有 tool_use 没有回应」结尾，之后每一次请求都会被服务商 400。
#[tauri::command]
pub async fn chat_fork(
    state: State<'_, AppState>,
    chat_id: String,
    topic_slug: Option<String>,
    to_slug: Option<String>,
    mode: Option<String>,
) -> AppResult<String> {
    let core = state.0.clone();
    // 互斥门包住「检查 + 整份读写」全段（本函数无 await，同步锁即可）：
    // 不然「检查通过 → start_turn 插进来落库 → 文件被复制」的交错会把
    // 半截历史复制进新对话
    let _gate = core.agent.chat_exclusive();
    ensure_safe_chat_id(&chat_id)?;
    // 分叉要整份读源文件：回合进行中读到的可能正好是「assistant(tool_use) 已落库、
    // tool_result 还没落库」的中间态，复制出去的新对话天然缺半截配对
    ensure_not_running(&core, &chat_id, "分叉")?;
    let mode = mode_of(mode.as_deref());
    let ws = core.workspace();

    // 工坊的对话没有主题：源目录与目标目录都是工作区级的 `.hub/chats/`
    if mode == AgentMode::Studio {
        let dir = crate::agent::chats_dir_for(&core, None);
        let src_path = dir.join(format!("{chat_id}.jsonl"));
        if !src_path.is_file() {
            return Err(AppError::NotFound(format!("工坊里没有这条对话：{chat_id}")));
        }
        let mut messages = crate::store::read_jsonl::<ChatMessage>(&src_path)?;
        // 源历史若有配对孤儿（进程死在工具执行窗口留下的），先清再分叉：
        // fork_prefix 原样保留到末尾，孤儿会被复制进新对话
        crate::agent::drop_orphan_tool_pairs(&mut messages);
        let src_title = crate::agent::chats::load(&dir, &chat_id).display_title(&messages);
        let kept = fork_prefix(&messages)?;
        let new_id = uuid::Uuid::new_v4().to_string();
        crate::agent::write_transcript(&core, None, &new_id, &kept)?;
        let mut meta = crate::agent::chats::ChatMeta {
            mode: AgentMode::Studio,
            ..Default::default()
        };
        meta.forked_from = Some(crate::agent::chats::ForkInfo {
            chat_id: chat_id.clone(),
            at_message: kept.len(),
            title: src_title,
        });
        crate::agent::chats::save(&dir, &new_id, &mut meta)?;
        return Ok(new_id);
    }

    // 从哪条对话分：优先用调用方给的主题，没给就按当前主题
    let from = match topic_slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) => ws.resolve(s)?,
        None => return Err(AppError::invalid("请先打开一个主题，再对里面的对话分叉")),
    };
    // 分到哪儿：默认还在同一条主题里
    let to = match to_slug.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(s) if s != from.slug() => ws.resolve(s)?,
        _ => from.clone(),
    };

    let src_dir = from.chats_dir();
    let src_path = from.chat_path(&chat_id);
    if !src_path.is_file() {
        return Err(AppError::NotFound(format!("这个主题里没有这条对话：{chat_id}")));
    }
    let mut messages = crate::store::read_jsonl::<ChatMessage>(&src_path)?;
    // 源历史若有配对孤儿（进程死在工具执行窗口留下的），先清再分叉：
    // fork_prefix 原样保留到末尾，孤儿会被复制进新对话
    crate::agent::drop_orphan_tool_pairs(&mut messages);
    let src_meta = crate::agent::chats::load(&src_dir, &chat_id);
    let src_title = src_meta.display_title(&messages);

    let kept = fork_prefix(&messages)?;
    let at = kept.len();

    let new_id = uuid::Uuid::new_v4().to_string();
    crate::agent::write_transcript(&core, Some(&to), &new_id, &kept)?;
    let mut meta = crate::agent::chats::ChatMeta::default();
    meta.forked_from = Some(crate::agent::chats::ForkInfo {
        chat_id: chat_id.clone(),
        at_message: at,
        title: src_title.clone(),
    });
    crate::agent::chats::save(&to.chats_dir(), &new_id, &mut meta)?;

    core.emit_topics_updated(&to);
    Ok(new_id)
}

/// 分叉时保留到哪：**最后一条 assistant 消息（含）+ 它后面成对的 tool_result**。
///
/// 只截到 assistant 的话，新对话会以「带 tool_use 却没有回应」的消息收尾——
/// 这条对话之后每一次请求都会被服务商 400（见 agent 主循环里关于配对落库的注释）。
/// assistant 之后再挂着的 user 消息照旧切掉：那是「用户刚提问、模型还没答」。
fn fork_prefix(messages: &[ChatMessage]) -> AppResult<Vec<ChatMessage>> {
    let at = messages
        .iter()
        .rposition(|m| m.role == crate::agent::message::Role::Assistant)
        .map(|i| i + 1)
        .unwrap_or(messages.len());
    let mut end = at;
    while end < messages.len() && messages[end].role == crate::agent::message::Role::Tool {
        end += 1;
    }
    let kept: Vec<ChatMessage> = messages[..end].to_vec();
    if kept.is_empty() {
        return Err(AppError::invalid(
            "这条对话还没有可复制的回合（至少要有一轮问答再分叉）",
        ));
    }
    Ok(kept)
}

/// chat_id 由前端/远程直接传进来、原样拼进文件名（`<id>.jsonl`）：
/// 带分隔符或 `..` 就能把删除 / 搬移 / 分叉指到工作区之外的文件。正常 id 都是 uuid。
/// 与 `transcript_path` / `meta_path` 的既有防护同一套，缺了就是路径穿越
///（这几条命令在远程白名单里，不能指望前端自觉）。
fn ensure_safe_chat_id(chat_id: &str) -> AppResult<()> {
    if crate::paths::is_safe_slug(chat_id) {
        Ok(())
    } else {
        Err(AppError::invalid(format!("对话 id 不合法：{chat_id}")))
    }
}

/// 这条对话还有回合在跑就不许动。回合是「每条消息现场追加」的（打开→写→关），
/// 中途把文件拿走，下一次追加会在原路径把文件重新建出来：删除的对话「复活」成
/// 一条只有尾巴的残件（无用户消息、无侧车元数据），搬移则在源主题留下同 id 的
/// 半截副本。等一轮收尾（或点「停止」）再整理，动作天然是安全的。
fn ensure_not_running(
    core: &std::sync::Arc<crate::state::AppCore>,
    chat_id: &str,
    action: &str,
) -> AppResult<()> {
    if core.agent.is_chat_running(chat_id) {
        return Err(AppError::invalid(format!(
            "这条对话还有一轮正在进行，等它跑完（或点「停止」）再{action}"
        )));
    }
    Ok(())
}

/// 分叉到别的主题时前端要切过去（见 `chat_fork` 的 `to_slug`）。
/// 把一条对话移进回收站（两侧车文件一起）。
///
/// 学习记录误删的代价高，所以和别处一样**不真删**：整个文件先进 `.hub/trash/`。
#[tauri::command]
pub async fn chat_delete(
    state: State<'_, AppState>,
    chat_id: String,
    topic_slug: Option<String>,
) -> AppResult<()> {
    let core = state.0.clone();
    // 互斥门包住「检查 + 移进回收站」全段（本函数无 await，同步锁即可）：
    // 检查与文件操作之间不再给 start_turn 插进来的机会，否则删除后下一次追加
    // 会在原路径把文件重建出来，删除的对话「复活」成只有尾巴的残件
    let _gate = core.agent.chat_exclusive();
    ensure_safe_chat_id(&chat_id)?;
    ensure_not_running(&core, &chat_id, "删除")?;
    let dir = crate::agent::chats_dir_for(&core, topic_slug.as_deref());
    let file = dir.join(format!("{chat_id}.jsonl"));
    if !file.is_file() {
        return Err(AppError::NotFound("这条对话已经不在磁盘上了".into()));
    }
    let trash = core
        .workspace()
        .root
        .join(crate::domain::topic::DIR_INTERNAL)
        .join("trash");
    crate::store::move_to_trash(&trash, &file)?;
    // 侧车文件（名字/置顶）跟着删：对话本身能从回收站还原，名字再起一个就是了
    crate::agent::chats::remove(&dir, &chat_id);
    core.emit_topics(crate::agent::event::TopicsEvent::Refresh);
    Ok(())
}

/// 把一条对话挪到另一个主题（侧栏里拖着整理：父主题 ↔ 子主题）。
///
/// 只搬 `.hub/chats/<id>.jsonl` 这一个文件：对话本身不依赖主题里的其它东西，
/// 里面的引用路径仍然是相对的，换主题后含义会变——所以移动是「整理」语义，
/// 不是「重新归因」，界面上给个提示即可。
#[tauri::command]
pub async fn chat_move(
    state: State<'_, AppState>,
    chat_id: String,
    from_slug: String,
    to_slug: String,
) -> AppResult<()> {
    let core = state.0.clone();
    // 互斥门包住「检查 + rename」全段（本函数无 await，同步锁即可）：检查通过后
    // 不再让 start_turn 插进来落库，否则搬走文件后下一次追加会在源主题原路径
    // 重建出同 id 的半截副本
    let _gate = core.agent.chat_exclusive();
    ensure_safe_chat_id(&chat_id)?;
    ensure_not_running(&core, &chat_id, "搬动")?;
    let ws = core.workspace();
    if from_slug == to_slug {
        return Ok(());
    }
    let from = ws.resolve(&from_slug)?;
    let to = ws.resolve(&to_slug)?;
    let src = from.chat_path(&chat_id);
    if !src.is_file() {
        return Err(AppError::NotFound(format!("这个主题里没有这条对话：{chat_id}")));
    }
    crate::paths::ensure_dir(&to.chats_dir())?;
    let dst = to.chat_path(&chat_id);
    if dst.exists() {
        return Err(AppError::invalid("目标主题里已经有一条同名对话了"));
    }
    std::fs::rename(&src, &dst)?;
    // 元数据（名字/置顶/归档/分叉血缘）跟着一起走，不然搬完就「失忆」了
    let src_meta = crate::agent::chats::meta_path(&from.chats_dir(), &chat_id);
    if src_meta.is_file() {
        let dst_meta = crate::agent::chats::meta_path(&to.chats_dir(), &chat_id);
        if let Err(e) = std::fs::rename(&src_meta, &dst_meta) {
            // 元数据搬不动不该让整个搬运动作失败：对话本身已经过去了，名字丢了大不了重起
            eprintln!("[chats] 搬对话时元数据没跟着走：{e}");
        }
    }
    core.emit_topics_updated(&from);
    core.emit_topics_updated(&to);
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RunningTurn {
    pub turn_id: String,
    pub running_ms: i64,
}

#[tauri::command]
pub async fn agent_running(state: State<'_, AppState>) -> AppResult<Vec<RunningTurn>> {
    Ok(state
        .0
        .agent
        .running_turns()
        .into_iter()
        .map(|t| RunningTurn { turn_id: t.turn_id, running_ms: t.started_at_ms })
        .collect())
}

#[cfg(test)]
mod fork_tests {
    use super::*;
    use crate::agent::message::{ContentBlock, Role};

    #[test]
    fn keeps_paired_tool_results_after_last_assistant() {
        // 「中止一轮之后分叉」是最常见的分叉场景：最后一条 assistant 带 tool_use，
        // 后面跟着补写的「已被用户中止」结果。这些结果必须跟着走——切掉的话
        // 新对话以孤儿 tool_use 收尾，之后每次请求都被服务商 400。
        let messages = vec![
            ChatMessage::user("帮我查一下"),
            ChatMessage::assistant(vec![ContentBlock::ToolUse {
                id: "tu1".into(),
                name: "viewer_read".into(),
                input: serde_json::json!({}),
            }]),
            ChatMessage::tool_result("tu1", "已被用户中止", true),
            ChatMessage::user("换个讲法"),
        ];
        let kept = fork_prefix(&messages).unwrap();
        assert_eq!(kept.len(), 3, "assistant 后面成对的 tool_result 要一起带走");
        assert_eq!(kept[0].role, Role::User);
        assert_eq!(kept[1].role, Role::Assistant);
        assert_eq!(kept[2].role, Role::Tool);
    }

    #[test]
    fn drops_trailing_unanswered_user_message() {
        // 末尾挂着「用户刚提问、模型还没答」：这句不能复制过去，
        // 否则新对话一开头就欠一个回答（分叉注释里写明的设计）。
        let messages = vec![
            ChatMessage::user("第一问"),
            ChatMessage::assistant_text("第一答"),
            ChatMessage::user("第二问"),
        ];
        let kept = fork_prefix(&messages).unwrap();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].role, Role::Assistant);
    }

    #[test]
    fn empty_history_is_rejected() {
        // 完全空的历史没有可复制的回合。
        // 注意「只有 user、没有 assistant」的历史**不**在拒绝之列：rposition 找不到
        // assistant 时截断点落在末尾，kept = 整份历史——这是改前就有的语义
        //（fork 的是「还没得到回答的提问」本身），这里不改变它，只钉住不回退。
        assert!(fork_prefix(&[]).is_err());
        let messages = vec![ChatMessage::user("只有一句提问")];
        let kept = fork_prefix(&messages).unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].role, Role::User);
    }
}
