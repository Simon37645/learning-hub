//! 长期记忆：把「用户是什么样的人、以后要注意什么」记下来，跨对话、跨主题复用。
//!
//! 为什么自己写而不接一套现成的记忆服务（mem0 / Letta / Zep 之类）：
//! 1. 本项目的寿命以「年」计，磁盘上的普通文件才是最耐久的接口——
//!    记忆必须能被用户用记事本打开、手动改、随手删，也要能跟着工作区一起同步；
//! 2. 那些方案的存量大头在「向量检索选哪几条记忆」。个人学习场景里记忆条数在几十条量级，
//!    全部塞进系统提示词反而更准（模型一眼看到全貌，不会漏掉关键的一条）；
//! 3. 少一个外部依赖 + 一个向量库 + 一套 API Key 要维护。
//!
//! 存储格式与工作区里其它东西一致（JSONL，一行一条）：
//!
//! ```text
//! <工作区>/.hub/memory/memories.jsonl        ← 全局（跨主题都成立：称呼、习惯、身体/作息、工具偏好）
//! <工作区>/<主题>/.hub/memory/memories.jsonl ← 本主题（这个主题里的薄弱点、说好的讲法、踩过的坑）
//! ```
//!
//! 作用域（scope）**由文件位置决定**，不写进记录里——同一份文件搬到别处语义就变了，
//! 记两个地方迟早会不一致。主题级的记忆沿父子链继承（学「某一章」时，
//! 整门课里记下的「他总把 A 和 B 搞混」照样该生效）。
//!
//! 注入策略：每次组装系统提示词时取一份简化版（分类 + 内容 + 来源日期），
//! 条数与字符数都有上限，防止提示词把上下文预算吃光。

use crate::error::{AppError, AppResult};
use crate::store;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// 应用内部目录名（`.hub` 相关路径由 [`global_path`] / [`topic_path`] 拼出）。
pub const INTERNAL_DIR: &str = ".hub";
/// 记忆文件在工作区 / 主题内的相对路径
pub const MEMORY_REL: &str = ".hub/memory/memories.jsonl";
/// 全局作用域在缓存表里的键（主题用的是 slug，不会撞）
pub const GLOBAL_KEY: &str = "*";
/// 单个作用域最多保留多少条：到了上限要提醒用户清理，而不是无声截断
pub const MAX_PER_SCOPE: usize = 60;
/// 注入提示词的字符预算
pub const DIGEST_BUDGET: usize = 1600;
/// 注入提示词的最大条数
pub const DIGEST_MAX_ITEMS: usize = 40;

/// 一条记忆的分类。分类不是装饰：提示词里按类分组，模型一眼知道该怎么用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// 关于用户的事实：专业、年级、基础、在用哪本教材
    #[default]
    Fact,
    /// 偏好：喜欢先看例子再给定义、不要一次给太多……
    Preference,
    /// 目标：考试的日期、想达到的水平
    Goal,
    /// 踩过的坑 / 需要注意的地方：易混点、老忘的地方、曾经被误导的结论
    Pitfall,
    /// 讲法与格式约定：公式要标单位、代码要给完整可运行版本
    Style,
    /// 知识缺口：还没学过的前置，需要补
    Gap,
}

impl MemoryKind {
    pub const ALL: [MemoryKind; 6] = [
        MemoryKind::Fact,
        MemoryKind::Preference,
        MemoryKind::Goal,
        MemoryKind::Pitfall,
        MemoryKind::Style,
        MemoryKind::Gap,
    ];

    pub fn slug(self) -> &'static str {
        match self {
            MemoryKind::Fact => "fact",
            MemoryKind::Preference => "preference",
            MemoryKind::Goal => "goal",
            MemoryKind::Pitfall => "pitfall",
            MemoryKind::Style => "style",
            MemoryKind::Gap => "gap",
        }
    }

    /// 界面与提示词里的中文名
    pub fn label(self) -> &'static str {
        match self {
            MemoryKind::Fact => "情况",
            MemoryKind::Preference => "偏好",
            MemoryKind::Goal => "目标",
            MemoryKind::Pitfall => "注意",
            MemoryKind::Style => "讲法",
            MemoryKind::Gap => "缺口",
        }
    }

    /// 给界面用的一句话说明
    pub fn hint(self) -> &'static str {
        match self {
            MemoryKind::Fact => "关于用户本人的情况：基础、专业、在用哪本书",
            MemoryKind::Preference => "学习方式的偏好：先例子还是先定义、讲多细",
            MemoryKind::Goal => "要达成的目标：考试时间、想学会什么",
            MemoryKind::Pitfall => "以后需要注意的地方：易混、老忘、容易走偏的点",
            MemoryKind::Style => "输出约定：单位、术语、代码风格、篇幅",
            MemoryKind::Gap => "还没掌握的前置知识，需要补课",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "fact" | "情况" | "事实" => Some(MemoryKind::Fact),
            "preference" | "偏好" => Some(MemoryKind::Preference),
            "goal" | "目标" => Some(MemoryKind::Goal),
            "pitfall" | "注意" | "坑" => Some(MemoryKind::Pitfall),
            "style" | "讲法" | "风格" => Some(MemoryKind::Style),
            "gap" | "缺口" | "薄弱" => Some(MemoryKind::Gap),
            _ => None,
        }
    }
}

/// 一条记忆。字段尽量少：多了就要人维护，人不会维护。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Memory {
    pub id: String,
    #[serde(default)]
    pub kind: MemoryKind,
    /// 一句话，写成可独立读懂的断言（「他把特征值和特征向量搞混」而不是「特征值」）
    pub content: String,
    /// 补充说明（记下当时的上下文），可以空
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// 钉住的记忆永远参与排序（放在最前面），也不会被自动清理
    #[serde(default)]
    pub pinned: bool,
    pub created_at: DateTime<Utc>,
    #[serde(default)]
    pub updated_at: Option<DateTime<Utc>>,
    /// 上一次被写进提示词的时间 + 次数：用来发现「记了但从来没起过作用」的条目
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
    #[serde(default)]
    pub use_count: u32,
}

impl Memory {
    pub fn new(kind: MemoryKind, content: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            kind,
            content: content.into(),
            note: String::new(),
            source: None,
            pinned: false,
            created_at: store::now(),
            updated_at: None,
            last_used: None,
            use_count: 0,
        }
    }

    /// 去重用的指纹：忽略空白与标点，只看内容本身。
    ///
    /// 模型每次都会重新措辞（「他喜欢…」/「用户喜欢…」），光靠全等去重会攒出一堆同义条，
    /// 所以这里把标点和空白都干掉再比。
    pub fn fingerprint(&self) -> String {
        self.content
            .chars()
            .filter(|c| !c.is_whitespace() && !is_punct(*c))
            .flat_map(|c| c.to_lowercase())
            .collect()
    }

    /// 界面与提示词里显示的时间：最后一次更新，没有就用创建时间
    pub fn stamp(&self) -> DateTime<Utc> {
        self.updated_at.unwrap_or(self.created_at)
    }
}

/// 半角与全角标点都算标点（模型爱用全角逗号句号）。
fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation()
        || matches!(
            c,
            '，' | '。' | '、' | '；' | '：' | '！' | '？' | '（' | '）' | '「' | '」' | '『' | '』'
                | '《' | '》' | '…' | '—' | '～' | '·' | '　'
        )
}

/// 一个作用域的视图（给前端与工具用）。
///
/// 字段名直接写成前端要的 camelCase：`Memory` 自己的 serde 规则与这里无关。
#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
pub struct MemoryView {
    pub id: String,
    pub kind: MemoryKind,
    pub content: String,
    pub note: String,
    pub source: Option<String>,
    pub pinned: bool,
    /// global / topic
    pub scope: String,
    /// 主题作用域下是主题 slug，全局是 null
    pub topic_slug: Option<String>,
    /// 主题作用域下是主题名（含「继承自父主题」的标注交给前端判断）
    pub topic_name: Option<String>,
    /// 这条记忆是从父主题继承来的（本主题只读清单里能看出来源）
    pub inherited: bool,
    pub createdAt: DateTime<Utc>,
    pub updatedAt: Option<DateTime<Utc>>,
    pub lastUsed: Option<DateTime<Utc>>,
    pub useCount: u32,
}

/// 记忆总览（面板用）。
#[derive(Debug, Clone, Serialize)]
#[allow(non_snake_case)]
pub struct MemoryOverview {
    /// 记忆功能总开关
    pub enabled: bool,
    /// 当前主题里是否真的会注入（没打开主题时只有全局记忆生效）
    pub topicName: Option<String>,
    /// 真正会写进系统提示词的条数（全局 + 本主题 + 父主题，且开着开关）
    pub activeCount: usize,
    /// 注入提示词的字符数
    pub digestChars: usize,
    /// 单个作用域的容量上限
    pub limit: usize,
    pub global: Vec<MemoryView>,
    /// 本主题自己的
    pub topic: Vec<MemoryView>,
    /// 从父主题继承来的（只读提示，删除要去父主题）
    pub inherited: Vec<MemoryView>,
}

/// 「一条记忆存在哪」——作用域由文件位置决定，所以这里只记位置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Global,
    Topic(String),
}

impl Scope {
    /// 缓存表里的键
    fn key(&self) -> &str {
        match self {
            Scope::Global => GLOBAL_KEY,
            Scope::Topic(slug) => slug,
        }
    }

    pub fn is_global(&self) -> bool {
        matches!(self, Scope::Global)
    }
}

/// 全局记忆文件：`<工作区>/.hub/memory/memories.jsonl`
pub fn global_path(workspace_root: &Path) -> PathBuf {
    workspace_root.join(".hub").join("memory").join("memories.jsonl")
}

/// 主题记忆文件：`<主题>/.hub/memory/memories.jsonl`
pub fn topic_path(topic_dir: &Path) -> PathBuf {
    topic_dir.join(".hub").join("memory").join("memories.jsonl")
}

/// 记忆仓库：全部记忆装在内存里，磁盘是唯一真相。
///
/// 为什么整份读进内存：个人用户的记忆量在几十到几百条，全量读 + 全量写几毫秒，
/// 换来的是「排序、去重、容量检查」都能当纯函数写、能脱离应用单测。
#[derive(Debug, Default)]
pub struct MemoryStore {
    /// 当前工作区根目录（换工作区时要整体重载）
    root: Option<PathBuf>,
    /// 缓存表：GLOBAL_KEY 或主题 slug → 该作用域的记忆
    scopes: BTreeMap<String, Vec<Memory>>,
    /// 有改动待落盘的作用域（注入提示词会更新「用了几次」，那种小改动攒着一起写）
    dirty: HashSet<String>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 切换到某个工作区：根目录变了就整体重载，没变就是空操作。
    ///
    /// 每次读记忆前都调一次是故意的——用户可能把工作区换到别的盘，
    /// 也可能直接用编辑器改了 jsonl 文件，这样一进来就是最新的。
    pub fn load(&mut self, workspace_root: &Path) -> AppResult<()> {
        let want = workspace_root.to_path_buf();
        if self.root.as_deref() == Some(want.as_path()) {
            return Ok(());
        }
        self.root = Some(want);
        self.scopes.clear();
        let global = store::read_jsonl::<Memory>(&global_path(workspace_root))?;
        if !global.is_empty() {
            self.scopes.insert(GLOBAL_KEY.to_string(), global);
        }
        self.dirty.clear();
        Ok(())
    }

    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// 把某个主题的记忆读进缓存（不存在就当空的）。重复调用是有意的：
    /// 用户可能直接用编辑器改了 jsonl，重新读一遍就能看到。
    ///
    /// 只读不写：如果这个作用域正攒着没落盘的「用了几次」，读盘会把它们丢掉，
    /// 所以先把它落盘再读。
    pub fn load_topic(&mut self, slug: &str, topic_dir: &Path) -> AppResult<()> {
        if self.dirty.contains(slug) {
            self.persist_key(slug, Some(topic_dir))?;
        }
        let items = store::read_jsonl::<Memory>(&topic_path(topic_dir))?;
        self.put(&Scope::Topic(slug.to_string()), items);
        Ok(())
    }

    /// 某个作用域的记忆清单（未排序，按磁盘顺序）。
    pub fn list(&self, scope: &Scope) -> Vec<Memory> {
        self.scopes.get(scope.key()).cloned().unwrap_or_default()
    }

    /// 主题作用域所在的文件。主题 slug **就是目录名**（见 ARCHITECTURE「主题与章节」），
    /// 所以缺目录时可以直接从工作区根还原——调用方不必每处都传目录。
    fn scope_file(&self, scope: &Scope, topic_dir: Option<&Path>) -> AppResult<PathBuf> {
        match scope {
            Scope::Global => {
                let root = self
                    .root
                    .as_ref()
                    .ok_or_else(|| AppError::other("记忆仓库还没绑定工作区"))?;
                Ok(global_path(root))
            }
            Scope::Topic(slug) => match topic_dir {
                Some(dir) => Ok(topic_path(dir)),
                None => {
                    let root = self
                        .root
                        .as_ref()
                        .ok_or_else(|| AppError::other("记忆仓库还没绑定工作区"))?;
                    Ok(topic_path(&root.join(slug)))
                }
            },
        }
    }

    /// 收下一批记忆（已经过 [`merge_into`] 处理），保持容量上限。
    fn put(&mut self, scope: &Scope, items: Vec<Memory>) {
        if items.is_empty() {
            self.scopes.remove(scope.key());
        } else {
            self.scopes.insert(scope.key().to_string(), items);
        }
    }

    /// 写入一条记忆并立刻落盘。返回 (最终记录, 是不是新增的)。
    ///
    /// `merge_into` 会先按内容指纹去重：同一条内容再次写入时更新它而不是新增一条，
    /// 免得模型每轮都「学到」同一件事，把提示词塞满重复内容。
    pub fn upsert(
        &mut self,
        scope: &Scope,
        topic_dir: Option<&Path>,
        mut item: Memory,
    ) -> AppResult<(Memory, bool)> {
        let path = self.scope_file(scope, topic_dir)?;
        let mut items = self.list(scope);
        let outcome = merge_into(&mut items, &mut item);
        if matches!(outcome, MergeOutcome::Added) && items.len() > MAX_PER_SCOPE {
            return Err(AppError::invalid(format!(
                "这个范围的记忆已经有 {MAX_PER_SCOPE} 条上限了。\
                 请先合并同类项，或在「记忆」面板里删掉不再需要的条目。"
            )));
        }
        self.put(scope, items);
        self.persist(scope, &path)?;
        Ok((item, matches!(outcome, MergeOutcome::Added)))
    }

    /// 按 id 局部更新（改内容 / 分类 / 钉住 / 备注）。找不到就报错，不静默新增。
    ///
    /// `note` 与前面几个参数一样是「给了才改」——传 `None` 表示不动原来的备注。
    pub fn patch(
        &mut self,
        scope: &Scope,
        topic_dir: Option<&Path>,
        id: &str,
        kind: Option<MemoryKind>,
        content: Option<String>,
        pinned: Option<bool>,
        note: Option<String>,
    ) -> AppResult<Memory> {
        let path = self.scope_file(scope, topic_dir)?;
        let mut items = self.list(scope);
        let slot = items
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or_else(|| AppError::NotFound(format!("没有 id 为 {id} 的记忆")))?;
        if let Some(k) = kind {
            slot.kind = k;
        }
        if let Some(c) = content {
            let c = c.trim();
            if c.is_empty() {
                return Err(AppError::invalid("记忆内容不能为空"));
            }
            slot.content = c.to_string();
        }
        if let Some(p) = pinned {
            slot.pinned = p;
        }
        if let Some(n) = note {
            slot.note = n.trim().to_string();
        }
        slot.updated_at = Some(store::now());
        let out = slot.clone();
        self.put(scope, items);
        self.persist(scope, &path)?;
        Ok(out)
    }

    /// 删除：给 id 就按 id 删，给内容就按内容精确匹配删（两者都没有则报错）。
    ///
    /// 为什么保留「按内容删」：模型手上常常只有自己刚写下的那句话，没有 id；
    /// 硬要求先 list 再删反而更容易删错。但**绝不做模糊匹配**——删错了没法撤销。
    pub fn forget(
        &mut self,
        scope: &Scope,
        topic_dir: Option<&Path>,
        ids: &[String],
        content: Option<&str>,
    ) -> AppResult<usize> {
        if ids.is_empty() && content.map(|c| c.trim().is_empty()).unwrap_or(true) {
            return Err(AppError::invalid("请给出要删除的记忆 id，或原样给出要删的内容"));
        }
        let path = self.scope_file(scope, topic_dir)?;
        let mut items = self.list(scope);
        let before = items.len();
        let wanted: HashSet<&str> = ids.iter().map(|s| s.as_str()).collect();
        let needle = content.map(|c| c.trim().to_string());
        items.retain(|m| {
            if wanted.contains(m.id.as_str()) {
                return false;
            }
            match &needle {
                Some(n) => m.content.trim() != n,
                None => true,
            }
        });
        let removed = before - items.len();
        if removed > 0 {
            self.put(scope, items);
            self.persist(scope, &path)?;
        }
        Ok(removed)
    }

    /// 清空某个作用域（界面上「清空」按钮 + 单元测试用）。返回删掉的条数。
    pub fn clear(
        &mut self,
        scope: &Scope,
        topic_dir: Option<&Path>,
    ) -> AppResult<usize> {
        let path = self.scope_file(scope, topic_dir)?;
        let n = self.list(scope).len();
        self.put(scope, Vec::new());
        if n > 0 {
            self.persist(scope, &path)?;
        }
        Ok(n)
    }

    /// 把「被注入过」这件事记下来（次数 +1、时间戳刷新），攒够一次一起落盘。
    ///
    /// 每次组装提示词都会走这里。逐条写盘会让对话开始多出几十次文件写操作，所以
    /// 只在内存里改，等 [`MemoryStore::flush`] 或下一次真正的写入时一起落盘。
    pub fn touch(&mut self, keys: &[(Scope, String)]) {
        if keys.is_empty() {
            return;
        }
        let now = store::now();
        for (scope, id) in keys {
            if let Some(items) = self.scopes.get_mut(scope.key()) {
                if let Some(m) = items.iter_mut().find(|m| &m.id == id) {
                    m.last_used = Some(now);
                    m.use_count = m.use_count.saturating_add(1);
                    self.dirty.insert(scope.key().to_string());
                }
            }
        }
    }

    /// 把攒着的小改动落盘（切换主题、关窗口前调用）。
    ///
    /// `topics` 给出 slug → 主题目录的对应关系（用来还原主题记忆文件的位置）。
    /// 还原不出来（主题被删了）就跳过——留一份孤儿文件比报错好。
    pub fn flush(&mut self, topics: &[(String, PathBuf)]) -> AppResult<()> {
        if self.dirty.is_empty() {
            return Ok(());
        }
        let keys: Vec<String> = self.dirty.iter().cloned().collect();
        for key in keys {
            let dir = topics
                .iter()
                .find(|(slug, _)| slug == &key)
                .map(|(_, d)| d.as_path());
            self.persist_key(&key, dir)?;
        }
        Ok(())
    }

    /// 把某个作用域的内存内容写到它的文件里；主题目录还原不出来就跳过。
    fn persist_key(&mut self, key: &str, topic_dir: Option<&Path>) -> AppResult<()> {
        let path = if key == GLOBAL_KEY {
            let Some(root) = self.root.clone() else { return Ok(()) };
            global_path(&root)
        } else {
            match topic_dir {
                Some(dir) => topic_path(dir),
                None => return Ok(()),
            }
        };
        let items = self.scopes.get(key).cloned().unwrap_or_default();
        store::write_jsonl(&path, &items)?;
        self.dirty.remove(key);
        Ok(())
    }

    fn persist(&mut self, scope: &Scope, path: &Path) -> AppResult<()> {
        let items = self.list(scope);
        store::write_jsonl(path, &items)?;
        self.dirty.remove(scope.key());
        Ok(())
    }

    /// 按优先级挑出要注入的条目（不碰任何状态）。
    ///
    /// `scopes` 按「优先级从高到低」给出（本主题 → 父主题 → 全局）；
    /// 同一条内容只挑一次，钉住的排在前面，字符与条数都在这里收口。
    fn pick(&self, scopes: &[(Scope, String)]) -> Vec<(Scope, Memory)> {
        let mut picked: Vec<(Scope, Memory)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        let mut chars = 0usize;

        for (scope, _label) in scopes {
            let mut items = self.list(scope);
            // 钉住的优先；其余按「用得多的、新的」排前面
            items.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then(b.use_count.cmp(&a.use_count))
                    .then(b.stamp().cmp(&a.stamp()))
            });
            for m in items {
                // 同一条内容在父主题和本主题各记了一份时只注入一次
                let fp = m.fingerprint();
                if !seen.insert(fp) {
                    continue;
                }
                let cost = m.content.chars().count() + 12;
                if chars + cost > DIGEST_BUDGET || picked.len() >= DIGEST_MAX_ITEMS {
                    break;
                }
                chars += cost;
                picked.push((scope.clone(), m));
            }
        }
        picked
    }

    /// 组装给系统提示词的一段文本；`scopes` 按「优先级从高到低」给出（本主题 → 父主题 → 全局）。
    ///
    /// 返回 (文本, 被用到的条数, 被用到的条目)。文本为空表示这个上下文里没有记忆可用。
    /// 这里会**就地更新** last_used / use_count —— 记忆不是收藏夹，
    /// 「记了但从来没被用到」应当在界面上看得出来。
    pub fn digest(&mut self, scopes: &[(Scope, String)]) -> (String, usize, Vec<(Scope, String)>) {
        let picked = self.pick(scopes);
        if picked.is_empty() {
            return (String::new(), 0, Vec::new());
        }
        let used: Vec<(Scope, String)> = picked
            .iter()
            .map(|(s, m)| (s.clone(), m.id.clone()))
            .collect();
        self.touch(&used);
        let text = render(&picked);
        (text, used.len(), used)
    }

    /// 只渲染不记账：给「预览系统提示词」这类只看不改的场景用。
    pub fn digest_text(&self, scopes: &[(Scope, String)]) -> String {
        render(&self.pick(scopes))
    }

    /// 供界面显示用的字符数估算（不真正注入、不改计数）。
    pub fn digest_preview(&self, scopes: &[(Scope, String)]) -> (usize, usize) {
        let mut seen: HashSet<String> = HashSet::new();
        let mut chars = 0usize;
        let mut n = 0usize;
        for (scope, _) in scopes {
            for m in self.list(scope) {
                if !seen.insert(m.fingerprint()) {
                    continue;
                }
                let cost = m.content.chars().count() + 12;
                if chars + cost > DIGEST_BUDGET || n >= DIGEST_MAX_ITEMS {
                    break;
                }
                chars += cost;
                n += 1;
            }
        }
        (n, chars)
    }

    /// 总览视图。`topic` 是当前主题（没有就只给全局）。
    pub fn overview(
        &self,
        enabled: bool,
        topic: Option<(&str, &str, Vec<String>)>,
    ) -> MemoryOverview {
        let global = to_views(&self.list(&Scope::Global), "global", None, None, false);
        let (topic_name, own, inherited, active_count, digest_chars) = match topic {
            Some((slug, name, ancestors)) => {
                let own = to_views(
                    &self.list(&Scope::Topic(slug.to_string())),
                    "topic",
                    Some(slug),
                    Some(name),
                    false,
                );
                let mut inherited = Vec::new();
                for a in &ancestors {
                    inherited.extend(to_views(
                        &self.list(&Scope::Topic(a.clone())),
                        "topic",
                        Some(a),
                        Some(a),
                        true,
                    ));
                }
                let mut scopes = vec![(Scope::Topic(slug.to_string()), name.to_string())];
                for a in &ancestors {
                    scopes.push((Scope::Topic(a.clone()), a.clone()));
                }
                scopes.push((Scope::Global, String::new()));
                let (n, chars) = self.digest_preview(&scopes);
                (Some(name.to_string()), own, inherited, n, chars)
            }
            None => {
                let (n, chars) = self.digest_preview(&[(Scope::Global, String::new())]);
                (None, Vec::new(), Vec::new(), n, chars)
            }
        };

        MemoryOverview {
            enabled,
            topicName: topic_name,
            activeCount: if enabled { active_count } else { 0 },
            digestChars: if enabled { digest_chars } else { 0 },
            limit: MAX_PER_SCOPE,
            global,
            topic: own,
            inherited,
        }
    }
}

/// 把挑好的条目渲染成提示词里的那一段。
///
/// 按分类分组输出：模型读起来比一长串平铺的句子清楚得多。
/// 每条带上「哪个主题记的」和日期——日期很重要，模型据此判断这条还成不成立。
fn render(picked: &[(Scope, Memory)]) -> String {
    if picked.is_empty() {
        return String::new();
    }
    let mut out = String::new();
    for kind in MemoryKind::ALL {
        let group: Vec<&(Scope, Memory)> = picked.iter().filter(|(_, m)| m.kind == kind).collect();
        if group.is_empty() {
            continue;
        }
        out.push_str(&format!("**{}**\n", kind.label()));
        for (scope, m) in group {
            let from = match scope {
                Scope::Global => String::new(),
                Scope::Topic(slug) => format!("·{slug}"),
            };
            let date = m.stamp().format("%Y-%m-%d");
            let flag = if m.pinned { "★" } else { "" };
            out.push_str(&format!("- {}{}{}（{}）\n", flag, m.content, from, date));
        }
    }
    out
}

/// 合并结果：是新增了一条，还是更新了已有的一条。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MergeOutcome {
    Added,
    Updated,
}

/// 把一条记忆合进列表：内容指纹相同就更新它（保留 id 与创建时间），否则追加。
fn merge_into(items: &mut Vec<Memory>, item: &mut Memory) -> MergeOutcome {
    let fp = item.fingerprint();
    if let Some(existing) = items.iter_mut().find(|m| m.fingerprint() == fp) {
        existing.kind = item.kind;
        existing.note = std::mem::take(&mut item.note);
        existing.source = item.source.clone();
        existing.pinned = existing.pinned || item.pinned;
        existing.updated_at = Some(store::now());
        *item = existing.clone();
        return MergeOutcome::Updated;
    }
    items.push(item.clone());
    MergeOutcome::Added
}

fn to_views(
    items: &[Memory],
    scope: &str,
    slug: Option<&str>,
    name: Option<&str>,
    inherited: bool,
) -> Vec<MemoryView> {
    let mut sorted = items.to_vec();
    sorted.sort_by(|a, b| {
        b.pinned
            .cmp(&a.pinned)
            .then(b.stamp().cmp(&a.stamp()))
    });
    sorted
        .into_iter()
        .map(|m| MemoryView {
            id: m.id,
            kind: m.kind,
            content: m.content,
            note: m.note,
            source: m.source,
            pinned: m.pinned,
            scope: scope.to_string(),
            topic_slug: slug.map(|s| s.to_string()),
            topic_name: name.map(|s| s.to_string()),
            inherited,
            createdAt: m.created_at,
            updatedAt: m.updated_at,
            lastUsed: m.last_used,
            useCount: m.use_count,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lh-memory-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn store_with(root: &Path) -> MemoryStore {
        let mut s = MemoryStore::new();
        s.load(root).unwrap();
        s
    }

    /// 全局记忆写在 `<工作区>/.hub/memory/memories.jsonl`，
    /// 主题记忆写在主题自己的 `.hub/memory/memories.jsonl`——两条路径不能互相污染。
    #[test]
    fn writes_to_scope_specific_files() {
        let root = tmp();
        let topic = root.join("线性代数");
        std::fs::create_dir_all(&topic).unwrap();
        let mut store = store_with(&root);

        store
            .upsert(
                &Scope::Global,
                None,
                Memory::new(MemoryKind::Preference, "喜欢先看例子再给定义"),
            )
            .unwrap();
        store
            .upsert(
                &Scope::Topic("线性代数".into()),
                Some(&topic),
                Memory::new(MemoryKind::Pitfall, "总把特征值和特征向量搞混"),
            )
            .unwrap();

        assert!(global_path(&root).exists(), "全局记忆文件没落盘");
        assert!(topic_path(&topic).exists(), "主题记忆文件没落盘");
        // 重新加载：两边各一条，互不串门
        let mut fresh = store_with(&root);
        assert_eq!(fresh.list(&Scope::Global).len(), 1);
        fresh.load_topic("线性代数", &topic).unwrap();
        assert_eq!(fresh.list(&Scope::Topic("线性代数".into())).len(), 1);
        assert!(fresh.list(&Scope::Topic("别的主题".into())).is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    /// 同一件事重复写入只更新、不新增（模型每轮都可能「重新学到」同一件事）。
    #[test]
    fn upsert_dedupes_by_content() {
        let root = tmp();
        let mut store = store_with(&root);
        let (first, added1) = store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "医学专业大三"))
            .unwrap();
        assert!(added1);
        // 只有标点/空白不同 → 视为同一条
        let (second, added2) = store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "医学专业，大三"))
            .unwrap();
        assert!(!added2, "标点差异不该产生新条目");
        assert_eq!(first.id, second.id);
        assert_eq!(store.list(&Scope::Global).len(), 1);
        assert!(store.list(&Scope::Global)[0].updated_at.is_some());

        std::fs::remove_dir_all(&root).ok();
    }

    /// 注入顺序：本主题 → 父主题 → 全局；同一条内容只出现一次；用完记次数。
    #[test]
    fn digest_merges_scopes_and_tracks_usage() {
        let root = tmp();
        let mut store = store_with(&root);
        store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "全局事实"))
            .unwrap();
        store
            .upsert(
                &Scope::Topic("父主题".into()),
                None,
                Memory::new(MemoryKind::Pitfall, "父主题里记的易混点"),
            )
            .unwrap();
        // 父主题与本主题各记了同一件事 → 只注入一次
        store
            .upsert(
                &Scope::Topic("本主题".into()),
                None,
                Memory::new(MemoryKind::Pitfall, "父主题里记的易混点"),
            )
            .unwrap();

        let (text, n, used) = store.digest(&[
            (Scope::Topic("本主题".into()), "本主题".into()),
            (Scope::Topic("父主题".into()), "父主题".into()),
            (Scope::Global, String::new()),
        ]);
        assert_eq!(n, 2, "同一件事记在两个作用域里只该注入一次：{text}");
        assert!(text.contains("全局事实"));
        assert!(text.contains("父主题里记的易混点"));
        assert!(text.contains("**注意**"), "应按分类分组");
        assert_eq!(used.len(), 2);
        // 注入过就记上次数
        let item = store
            .list(&Scope::Global)
            .into_iter()
            .find(|m| m.content == "全局事实")
            .unwrap();
        assert_eq!(item.use_count, 1);

        std::fs::remove_dir_all(&root).ok();
    }

    /// 钉住的排在最前；字符预算用完就不再塞。
    #[test]
    fn digest_respects_pin_and_budget() {
        let root = tmp();
        let mut store = store_with(&root);
        for i in 0..10 {
            store
                .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, format!("普通条目{i}")))
                .unwrap();
        }
        let pin = Memory {
            pinned: true,
            ..Memory::new(MemoryKind::Goal, "六月要考执业医师")
        };
        store.upsert(&Scope::Global, None, pin).unwrap();

        // 预算调到只够放钉住的那条
        let long = "很长的一条记忆".repeat(40);
        store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, long))
            .unwrap();

        let (text, _, _) = store.digest(&[(Scope::Global, String::new())]);
        assert!(text.contains("六月要考执业医师"), "钉住的必须进提示词");
        assert!(text.chars().count() < DIGEST_BUDGET + 200, "预算没守住");

        std::fs::remove_dir_all(&root).ok();
    }

    /// 删除：按 id 与按原内容都能删；内容只能精确匹配，不做模糊。
    #[test]
    fn forget_by_id_and_content() {
        let root = tmp();
        let mut store = store_with(&root);
        let (a, _) = store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "第一条"))
            .unwrap();
        store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "第二条"))
            .unwrap();
        store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "第三条"))
            .unwrap();

        assert_eq!(
            store.forget(&Scope::Global, None, &[a.id.clone()], None).unwrap(),
            1
        );
        assert_eq!(
            store.forget(&Scope::Global, None, &[], Some("第二条")).unwrap(),
            1
        );
        // 模糊内容不匹配 → 一条都不删，也不报错
        assert_eq!(store.forget(&Scope::Global, None, &[], Some("第")).unwrap(), 0);
        assert_eq!(store.list(&Scope::Global).len(), 1);
        // 什么都不给是参数错误
        assert!(store.forget(&Scope::Global, None, &[], None).is_err());

        std::fs::remove_dir_all(&root).ok();
    }

    /// 容量上限：到顶之后新增会被拒绝并给出可操作的提示（而不是悄悄丢掉）。
    #[test]
    fn rejects_when_scope_is_full() {
        let root = tmp();
        let mut store = store_with(&root);
        for i in 0..MAX_PER_SCOPE {
            store
                .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, format!("条目 {i}")))
                .unwrap();
        }
        let err = store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "再来一条".to_string()))
            .unwrap_err();
        assert!(err.to_string().contains("上限"), "错误提示要说清怎么办：{err}");

        std::fs::remove_dir_all(&root).ok();
    }

    /// 换工作区要整体重载（不然会把上一个工作区的记忆带过去）。
    #[test]
    fn reloads_on_workspace_switch() {
        let a = tmp();
        let b = tmp();
        let mut store = store_with(&a);
        store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "工作区 A 的事"))
            .unwrap();
        store.load(&b).unwrap();
        assert!(store.list(&Scope::Global).is_empty(), "换工作区后应清空缓存");
        store
            .upsert(&Scope::Global, None, Memory::new(MemoryKind::Fact, "工作区 B 的事"))
            .unwrap();
        // A 的那条还在 A 的文件里，没被覆盖
        let mut back = store_with(&a);
        assert_eq!(back.list(&Scope::Global)[0].content, "工作区 A 的事");

        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }

    /// 父主题里记下的事，学「某一章」（子主题）时必须照样生效——
    /// 这正是「同一门课里他总把 A 和 B 搞混」应当被继承的场景。
    #[test]
    fn digest_inherits_from_ancestor_scopes() {
        let root = tmp();
        let mut store = store_with(&root);
        store
            .upsert(
                &Scope::Topic("父主题".into()),
                None,
                Memory::new(MemoryKind::Pitfall, "整门课里他总把 A 和 B 搞混"),
            )
            .unwrap();
        store
            .upsert(
                &Scope::Topic("本主题".into()),
                None,
                Memory::new(MemoryKind::Fact, "这一章他还没学过向量"),
            )
            .unwrap();

        // 子主题的上下文：本主题 → 父主题 → 全局
        let text = store.digest_text(&[
            (Scope::Topic("本主题".into()), "本主题".into()),
            (Scope::Topic("父主题".into()), "父主题".into()),
            (Scope::Global, String::new()),
        ]);
        assert!(text.contains("整门课里他总把 A 和 B 搞混"), "父主题的记忆没继承过来：{text}");
        assert!(text.contains("这一章他还没学过向量"));
        // 标出这条是哪个主题记的，模型才知道适用范围
        assert!(text.contains("·父主题"));

        std::fs::remove_dir_all(&root).ok();
    }

    /// 指纹去重要忽略空白、标点与大小写——模型每次都会换个说法。
    #[test]
    fn fingerprint_ignores_punctuation_and_case() {
        let a = Memory::new(MemoryKind::Fact, " He likes examples, first! ");
        let b = Memory::new(MemoryKind::Fact, "he likes examples first");
        assert_eq!(a.fingerprint(), b.fingerprint());
        let c = Memory::new(MemoryKind::Fact, "he likes theory first");
        assert_ne!(a.fingerprint(), c.fingerprint());
    }

    #[test]
    fn kind_parse_and_labels() {
        assert_eq!(MemoryKind::parse("注意"), Some(MemoryKind::Pitfall));
        assert_eq!(MemoryKind::parse("pitfall"), Some(MemoryKind::Pitfall));
        assert_eq!(MemoryKind::parse("不认识的"), None);
        assert_eq!(MemoryKind::Gap.label(), "缺口");
        // 六类都有中文名与说明，界面上不会出现空标签
        for k in MemoryKind::ALL {
            assert!(!k.label().is_empty() && !k.hint().is_empty());
        }
    }
}
