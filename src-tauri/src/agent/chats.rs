//! 对话的元数据：标题、置顶、归档、分叉血缘。
//!
//! 为什么不把这些塞进 `chats/<id>.jsonl`：
//! 那个文件的每一行都是一条 `ChatMessage`，模型读的、前端渲染的、后面可能要导出的都是它。
//! 往里插一行「特殊行」会让**每一处**读对话的地方都得先学会跳过它——漏一处就是一条假消息。
//! 所以元数据放**侧车文件** `chats/<id>.meta.json`，与 jsonl 同名同目录：
//! 搬对话、删对话都是一个目录里的两个文件，跟着一起走。
//!
//! 元数据是**可选**的：老对话没有侧车文件，标题就用第一句用户消息现算——
//! 用户不改名就永远不会有侧车文件，界面完全一样。
//!
//! 字段刻意少：标题、置顶、归档、分叉血缘。每多一个字段，用户就要多维护一件事。

use crate::agent::message::{ChatMessage, Role};
use crate::agent::provider::truncate;
use crate::error::AppResult;
use crate::store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// 标题长度上限：侧栏一行放不下更长，也不该让标题变成摘要
pub const TITLE_MAX: usize = 60;

/// 一条对话的元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatMeta {
    /// 用户自己起的名字；空串表示还没改过名（标题现算）
    #[serde(default)]
    pub title: String,
    /// 置顶：排在该主题对话列表的最前面
    #[serde(default)]
    pub pinned: bool,
    /// 归档：默认收进侧栏的「已归档」里，不再占日常视线
    #[serde(default)]
    pub archived: bool,
    #[serde(default)]
    pub created_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    /// 分叉来源：从哪条对话、哪个位置分出来的
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from: Option<ForkInfo>,
    /// 这条对话属于哪个模式：学习 / 工坊。
    ///
    /// 为什么记在这儿而不是另开一个目录：工作区级（没有主题）的对话本来就都存在
    /// `.hub/chats/` 下，id 是 uuid、不会撞。「首页的日常问答」和「工坊里造东西」
    /// 是两份清单，靠这一个字段分开——改名、置顶、归档、删除这些操作就都不用再写一遍。
    #[serde(default)]
    pub mode: crate::agent::registry::AgentMode,
}

/// 分叉血缘。只记「从哪来」，不记「分出去哪些」——
/// 反查要扫全部对话，而正查（这条是从哪来的）是界面上唯一要显示的东西。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkInfo {
    pub chat_id: String,
    /// 分叉点在原对话里的消息序号（0 基），便于以后做「跳回原对话」
    pub at_message: usize,
    /// 原对话当时的标题（原对话删了也还能看懂来历）
    #[serde(default)]
    pub title: String,
}

impl Default for ChatMeta {
    fn default() -> Self {
        Self {
            title: String::new(),
            pinned: false,
            archived: false,
            created_at: None,
            updated_at: None,
            forked_from: None,
            mode: crate::agent::registry::AgentMode::Study,
        }
    }
}

/// 元数据文件路径：`chats/<id>.meta.json`（对话本身是 `chats/<id>.jsonl`）。
pub fn meta_path(chats_dir: &Path, chat_id: &str) -> PathBuf {
    chats_dir.join(format!("{chat_id}.meta.json"))
}

/// 读元数据；没有侧车文件就返回默认值（不报错——老对话本来就没有）。
pub fn load(chats_dir: &Path, chat_id: &str) -> ChatMeta {
    match store::read_json_opt::<ChatMeta>(&meta_path(chats_dir, chat_id)) {
        Ok(Some(m)) => m,
        Ok(None) => ChatMeta::default(),
        Err(e) => {
            // 元数据坏了不该让整条对话打不开：退回默认值，把原因打到日志
            eprintln!("[chats] 读取 {chat_id} 的元数据失败，按默认处理：{e}");
            ChatMeta::default()
        }
    }
}

/// 写元数据。全默认值（没改名、没置顶、没归档、没血缘）时**删掉**侧车文件，
/// 免得磁盘上留下一堆空壳。
pub fn save(chats_dir: &Path, chat_id: &str, meta: &mut ChatMeta) -> AppResult<()> {
    let path = meta_path(chats_dir, chat_id);
    if meta.is_default() {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        return Ok(());
    }
    meta.updated_at = Some(store::now());
    store::write_json(&path, meta)
}

/// 删掉元数据（对话文件被搬走/删掉时跟着清理）。
pub fn remove(chats_dir: &Path, chat_id: &str) {
    let path = meta_path(chats_dir, chat_id);
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

impl ChatMeta {
    /// 什么都没设过——这种元数据不值得落盘。
    ///
    /// 例外：工坊的对话一定要留侧车（`mode` 就是它的身份），
    /// 否则它会被当成首页的日常问答，出现在另一份清单里。
    pub fn is_default(&self) -> bool {
        self.title.trim().is_empty()
            && !self.pinned
            && !self.archived
            && self.forked_from.is_none()
            && self.mode == crate::agent::registry::AgentMode::Study
    }

    /// 界面上真正显示的标题：用户起过名就用它，否则取第一句用户消息。
    pub fn display_title(&self, messages: &[ChatMessage]) -> String {
        let custom = self.title.trim();
        if !custom.is_empty() {
            return custom.to_string();
        }
        messages
            .iter()
            .find(|m| m.role == Role::User)
            // display_text 而不是 text：只贴了一张图没打字的对话也要有标题
            // （否则侧栏那一行是空的，看着像坏了）
            .map(|m| truncate(m.display_text().trim(), 40))
            .unwrap_or_default()
    }

    /// 改名。空标题视为「恢复自动标题」（把侧车里的 title 清空）。
    pub fn rename(&mut self, title: &str) {
        let t = title.trim();
        self.title = if t.is_empty() {
            String::new()
        } else {
            t.chars().take(TITLE_MAX).collect()
        };
    }
}

/// 侧栏列表要的一项。后端算好排序，前端只负责画。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatOverviewItem {
    pub id: String,
    /// 显示用标题（自定义优先，否则第一句用户消息）
    pub title: String,
    /// 用户自己起过名（界面上可以显示成不同样式，也能提示「清除自定义名」）
    pub custom_title: bool,
    pub messages: usize,
    pub updated_at: DateTime<Utc>,
    pub pinned: bool,
    pub archived: bool,
    pub forked_from: Option<ForkInfo>,
    /// 学习 / 工坊（侧栏据此把两条清单分开）
    pub mode: crate::agent::registry::AgentMode,
}

/// 排序：置顶在最前 → 归档沉到底 → 再按最近使用。
///
/// 侧栏每个主题下面只展开十来行，「置顶」是唯一能把常用的顶到眼前的手段。
pub fn sort_items(items: &mut [ChatOverviewItem]) {
    items.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(a.archived.cmp(&b.archived))
            .then(b.updated_at.cmp(&a.updated_at))
    });
}

/// 从「磁盘上有什么」+「消息内容」组装一项。纯函数，好测。
pub fn overview_item(
    id: String,
    messages: &[ChatMessage],
    meta: &ChatMeta,
    updated_at: DateTime<Utc>,
) -> ChatOverviewItem {
    ChatOverviewItem {
        title: meta.display_title(messages),
        custom_title: !meta.title.trim().is_empty(),
        id,
        messages: messages.len(),
        updated_at,
        pinned: meta.pinned,
        archived: meta.archived,
        forked_from: meta.forked_from.clone(),
        mode: meta.mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::message::ContentBlock;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lh-chats-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn item(id: &str, pinned: bool, archived: bool, minutes_ago: i64) -> ChatOverviewItem {
        ChatOverviewItem {
            id: id.into(),
            title: id.into(),
            custom_title: false,
            messages: 1,
            updated_at: store::now() - chrono::Duration::minutes(minutes_ago),
            pinned,
            archived,
            forked_from: None,
            mode: crate::agent::registry::AgentMode::Study,
        }
    }

    #[test]
    fn saves_only_when_something_is_set() {
        let dir = tmp();
        let mut meta = ChatMeta::default();
        save(&dir, "c1", &mut meta).unwrap();
        assert!(!meta_path(&dir, "c1").exists(), "全默认值不该落盘");

        meta.rename("特征值复习");
        save(&dir, "c1", &mut meta).unwrap();
        assert!(meta_path(&dir, "c1").exists());
        let back = load(&dir, "c1");
        assert_eq!(back.title, "特征值复习");
        assert!(back.updated_at.is_some());

        // 再改回默认（清空标题）→ 侧车文件应当被清掉
        meta.rename("   ");
        save(&dir, "c1", &mut meta).unwrap();
        assert!(!meta_path(&dir, "c1").exists(), "恢复自动标题后不该留空壳");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_or_broken_meta_falls_back_to_default() {
        let dir = tmp();
        assert!(!load(&dir, "nope").pinned);
        // 坏文件也不能让对话打不开
        std::fs::write(meta_path(&dir, "bad"), "{ 这不是 json").unwrap();
        let m = load(&dir, "bad");
        assert!(m.title.is_empty() && !m.archived);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn title_falls_back_to_first_user_message() {
        let messages = vec![
            ChatMessage::assistant_text("开场白"),
            ChatMessage::user("帮我讲讲特征值到底在说什么"),
            ChatMessage::user("再举个例子"),
        ];
        let mut meta = ChatMeta::default();
        assert_eq!(meta.display_title(&messages), "帮我讲讲特征值到底在说什么");
        meta.rename("  自定义名字  ");
        assert_eq!(meta.display_title(&messages), "自定义名字");
        // 长标题要截断，别把侧栏撑开
        meta.rename(&"长".repeat(200));
        assert_eq!(meta.title.chars().count(), TITLE_MAX);
    }

    /// 只贴了图、没打字的对话：标题不能是空的（侧栏那一行会看着像坏了）。
    #[test]
    fn title_falls_back_to_image_placeholder() {
        use crate::agent::message::ContentBlock;
        let messages = vec![ChatMessage::new(
            Role::User,
            vec![ContentBlock::Image {
                path: ".hub/attachments/a.png".into(),
                media_type: "image/png".into(),
                name: "a.png".into(),
                bytes: 10,
                width: None,
                height: None,
            }],
        )];
        let meta = ChatMeta::default();
        assert_eq!(meta.display_title(&messages), "（图片）");
    }

    /// 工坊的对话必须留下侧车（mode 就是它的身份），否则会跑进首页那份清单里。
    #[test]
    fn studio_chats_keep_their_meta() {
        let dir = tmp();
        let mut meta = ChatMeta {
            mode: crate::agent::registry::AgentMode::Studio,
            ..ChatMeta::default()
        };
        assert!(!meta.is_default(), "工坊对话不能算「什么都没设过」");
        save(&dir, "s1", &mut meta).unwrap();
        assert!(meta_path(&dir, "s1").exists());
        assert_eq!(load(&dir, "s1").mode, crate::agent::registry::AgentMode::Studio);

        // 老侧车文件里没有 mode 字段：要按学习模式读出来，而不是报错
        std::fs::write(meta_path(&dir, "old"), r#"{"title":"旧对话"}"#).unwrap();
        let old = load(&dir, "old");
        assert_eq!(old.mode, crate::agent::registry::AgentMode::Study);
        assert_eq!(old.title, "旧对话");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// 排序规则：置顶最前、归档沉底、其余按最近使用。
    #[test]
    fn sort_puts_pinned_first_and_archived_last() {
        let mut items = vec![
            item("普通旧", false, false, 100),
            item("归档新", false, true, 1),
            item("普通新", false, false, 1),
            item("置顶旧", true, false, 500),
        ];
        sort_items(&mut items);
        let ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["置顶旧", "普通新", "普通旧", "归档新"]);
    }

    #[test]
    fn overview_item_marks_custom_title_and_lineage() {
        let messages = vec![ChatMessage::new(Role::User, vec![ContentBlock::text("原始第一句")])];
        let mut meta = ChatMeta::default();
        let it = overview_item("c9".into(), &messages, &meta, store::now());
        assert_eq!(it.title, "原始第一句");
        assert!(!it.custom_title);

        meta.rename("改过的名字");
        meta.forked_from = Some(ForkInfo {
            chat_id: "parent".into(),
            at_message: 4,
            title: "原对话".into(),
        });
        let it = overview_item("c9".into(), &messages, &meta, store::now());
        assert_eq!(it.title, "改过的名字");
        assert!(it.custom_title);
        assert_eq!(it.forked_from.unwrap().at_message, 4);
    }
}
