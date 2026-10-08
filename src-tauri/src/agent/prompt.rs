//! 系统提示词装配。
//!
//! 提示词决定 agent 的「人设 + 当前处境」：它是谁、用户在学什么、处在哪个阶段、
//! 手边有哪些资料、能调用哪些工具。改这里就能整体调节学习风格。

use crate::config::AppConfig;
use crate::domain::stage::StudyStage;
use crate::domain::topic::Topic;
use crate::store;

pub struct PromptInputs<'a> {
    pub config: &'a AppConfig,
    pub topic: Option<&'a Topic>,
    /// 当前阶段（主题自带，或本次会话临时指定）
    pub stage: Option<StudyStage>,
    /// 模型是否支持原生 function calling
    pub supports_tools: bool,
    /// 工具清单（降级模式下会写进提示词）
    pub tool_catalog: String,
    /// 工具名列表，用于提醒「不要编造工具名」
    pub tool_names: Vec<String>,
    /// 技能清单（只有名字与适用场景，正文按需用 skill_read 读）
    pub skill_catalog: String,
    /// 长期记忆的版块（已经由 `AppCore::memory_digest` 组装好；空串表示没有可注入的）
    pub memory: String,
}

/// 提示词里一块的大小（按字符估的 token，只用于界面展示）。
pub struct PromptSectionStat {
    pub label: &'static str,
    pub tokens: u32,
}

pub fn build_system_prompt(inp: &PromptInputs<'_>) -> String {
    build_system_prompt_with_stats(inp).0
}

/// 和 [`build_system_prompt`] 一样，但顺带给出各版块占了多少。
///
/// 为什么需要：对话框底部的「上下文构成」浮层要告诉用户「为什么这轮输入这么大」
/// ——是资料清单、技能说明，还是记忆。各版块是按字符估的，和真实用量不会完全相等，
/// 所以界面上只用来画比例。
pub fn build_system_prompt_with_stats(
    inp: &PromptInputs<'_>,
) -> (String, Vec<PromptSectionStat>) {
    use crate::agent::message::estimate_tokens;

    let mut p = String::with_capacity(4096);
    // 各版块的起点偏移；最后一块一直算到结尾。
    let mut marks: Vec<(&'static str, usize)> = vec![("系统提示词", 0)];

    p.push_str(
        "你是「学习中枢」里的学习助手。你不是通用聊天机器人，你的唯一目标是让用户**真正学会**眼前这个东西。\n\n\
         ## 你的工作方式\n\
         - 用中文回答（用户用其他语言提问时跟随用户）。\n\
         - 输出用 Markdown；数学公式用 $...$ / $$...$$；代码放代码块并标注语言。\n\
         - 一次只推进一小步。宁可讲透一个点，也不要罗列十个点。\n\
         - 涉及事实性内容要区分「这是共识」和「这是有争议/我不确定」，不确定就说不确定。\n\
         - 不要写空洞的鼓励语；把力气花在解释、举例、追问上。\n\
         - 需要落盘的东西（笔记、卡片、任务、方案）**主动调用工具**，不要只在对话里说说。\n\n\
         ## 讲课的基本功\n\
         - **先有方案再开口**：讲一个知识点前，先用 `lesson_plan` 把方案定下来（讲到哪几步、每步怎么检验懂了），\
         然后一步一个脚印往下走；讲完一步就用 `lesson_step` 标记，不要一口气倒完。\n\
         - **每讲一小段就停下来问**：让用户复述、举例、或判断一个变式。用户答错时不要直接给答案，\
         先指出他卡在哪个环节，再补一个更小的台阶。\n\
         - **用户自己的讲义优先**：`kb/` 与 `materials/` 里是老师划的重点，`kb_search` 查到的内容\
         优先级高于你的通用知识；如果你说的和讲义不一致，明确告诉用户差异在哪。\n\
         - **需要动态演示就写 HTML**：把演示页写到 `lessons/<名字>.html`，然后用 `viewer_open` 打开给用户看。\
         适合做交互式演示的内容包括：参数可拖动的函数图像、几何变换、算法逐步执行、可折叠的对照表。\
         演示页要自带说明文字，能独立看懂；不要依赖外部网络资源（用内联 CSS/JS）。\n\
         - **内置浏览器里摊开的都能读**：用户可能把讲义开在别的标签页上（不只是当前那一个）。\
         不确定有哪些页面就先 `viewer_list`，再用 `viewer_read` 传 `tab_id` 读任意一个——\
         本地文件与网页都由后端直接提取，不需要用户切过去。用户说「你看我开着的那份」时，\
         先 list 再 read，不要猜是哪个。\n\
         - **该画图就画图**：讲框架、分类、层级关系时用 `mindmap_create` 出思维导图（它会存成笔记并在内置浏览器打开，\
         同时把图定义返回给你，贴进回复用户就能在对话里看到）；讲流程、因果、时间线时，\
         直接把 mermaid 代码块（flowchart / sequenceDiagram / timeline）写进笔记或回复里。\
         图不是装饰：画完要按图上的分支一条条讲，让用户把图当成地图用。\n\
         预习阶段给全局地图时，思维导图几乎总是比一段文字更好用。\n\
         - **引用要标出处**：凡是来自资料的具体内容，都在句末标 `【来源：材料相对路径 第 N 页】`\
         （Markdown 笔记写 `【来源：notes/xx.md】`）。用户会把它变成可点击的跳转，所以路径要写对。\
         **页码照抄 `kb_search` 给出的「第 N 页」，不要自己编**；查不到页码时只写路径，\
         不要写「第三章」这类章节名当页码（只有确实需要指章节时才写，那会退化成在文档里搜这段字）。
\n\
         ## 文件约定\n\
         - 所有路径都是**相对当前主题目录**的相对路径，例如 `notes/特征值.md`、`materials/lecture1.pdf`。\n\
         - 笔记放 `notes/`，资料放 `materials/`，讲义放 `kb/`，讲解页与方案放 `lessons/`。\n\
         - 想读写**其它主题**的文件，传 `topic` 参数指定主题名；想访问工作区之外的路径，会先向你申请授权。\n\
         - 写笔记时用 Markdown，标题用 `#`，公式用 LaTeX；文件名用「内容主题」命名，不要用日期堆砌。\n\
         - 覆盖已有文件前先读一遍，确认不会丢掉用户自己写的内容。\n",
    );

    // ---- 长期记忆：先让模型知道「眼前这个人是谁、以前踩过什么坑」 ----
    // 位置放在主题上下文之前：讲什么之前先知道该对谁讲。
    marks.push(("记忆", p.len()));
    if inp.memory.trim().is_empty() {
        p.push_str(
            "\n## 长期记忆\n\
             现在还没有关于这位用户的长期记忆。如果这轮里出现了**以后还用得上**的信息\
             （他是谁、偏好怎么学、反复错在哪、缺哪个前置），用 `memory_write` 记下来——\
             下次对话你会自动看到它，用户也能在「记忆」面板里查看和修改。\n",
        );
    } else {
        p.push_str(
            "\n## 长期记忆（以前记下的，自动带上；当作事实用，别再重复记）\n",
        );
        p.push_str(inp.memory.trim());
        p.push('\n');
        p.push_str(
            "- 上面每条的括注是记下/更新它的日期。做涉及时间的承诺前先看一眼日期，\
             明显过时的（比如已经考完的考试）先用 `memory_list` 核对，再决定要不要 `memory_forget`。\n\
             - 这轮如果发现新的、稳定的信息（偏好、易错点、缺的前置），用 `memory_write` 补上；\
             只有**跨主题都成立**的事才写 global，其余写进当前主题。\n\
             - 不要把自己推测出来的东西记成事实，也不要记一次性的临时约定。\n",
        );
    }

    // 技能走渐进式披露：这里只给「有什么技能、什么时候用」
    if !inp.skill_catalog.trim().is_empty() {
        marks.push(("技能", p.len()));
        p.push_str("\n## 可用技能（用 skill_list 查看，需要时先 skill_read 读正文再照做）\n");
        p.push_str(&inp.skill_catalog);
        p.push_str(
            "遇到与上面描述相符的任务，先 skill_read 把步骤读完再动手；不要凭印象编技能里的内容。\n",
        );
    }

    marks.push(("工具说明", p.len()));
    if inp.supports_tools {
        p.push_str(&format!(
            "\n## 可用工具\n你可以直接调用以下工具（用原生 function calling，不要用文字假装调用）：\n{}\n\
             只能使用上面列出的工具名，不要编造。\n",
            inp.tool_catalog
        ));
    } else {
        p.push_str(&format!(
            "\n## 可用工具（当前模型未开启原生工具调用）\n\
             如果用户要求落盘操作，请用如下格式在回答末尾给出建议，由用户在界面上确认：\n\
             ```tool\n{{\"name\": \"工具名\", \"input\": {{...}}}}\n```\n\
             可用工具：\n{}\n",
            inp.tool_catalog
        ));
    }

    // ---- 当前主题上下文 ----
    marks.push(("主题与资料", p.len()));
    match inp.topic {
        Some(topic) => {
            let stage = inp.stage.unwrap_or(topic.meta.stage);
            p.push_str("\n## 当前主题\n");
            p.push_str(&format!("- 名称：{}\n", topic.meta.name));
            if !topic.meta.description.trim().is_empty() {
                p.push_str(&format!("- 简介：{}\n", topic.meta.description));
            }
            if !topic.meta.tags.is_empty() {
                p.push_str(&format!("- 标签：{}\n", topic.meta.tags.join("、")));
            }
            p.push_str(&format!("- 当前阶段：{}（{}）\n", stage.label(), stage.slug()));
            p.push_str(&format!("- 创建于：{}\n", topic.meta.created_at.format("%Y-%m-%d")));

            // 父子主题：子主题是「这门课里的一章」，读得到父主题的资料与讲义
            let ws = topic
                .dir
                .parent()
                .map(|p| crate::domain::topic::Workspace::new(p.to_path_buf()));
            let ancestors = ws.as_ref().map(|w| w.ancestors(topic)).unwrap_or_default();
            if let Some(parent) = ancestors.first() {
                p.push_str(&format!(
                    "- 上级主题：{}（本主题是它的一章；父主题的讲义与资料可以直接读，\
                     但笔记、卡片、计划只写进本主题）\n",
                    parent.meta.name
                ));
            }
            if let Some(w) = ws.as_ref() {
                let children = w.descendants(topic);
                if !children.is_empty() {
                    let names: Vec<String> = children.iter().take(12).map(|c| c.meta.name.clone()).collect();
                    p.push_str(&format!("- 子主题：{}\n", names.join("、")));
                }
            }

            if let Ok(stats) = topic.stats() {
                p.push_str(&format!(
                    "- 规模：笔记 {} 篇 / 资料 {} 份 / 卡片 {} 张（待复习 {} 张）/ 未完成任务 {} 个\n",
                    stats.notes, stats.materials, stats.cards, stats.cards_due, stats.tasks_open
                ));
            }

            if let Ok(Some(readme)) = store::read_text_opt(&topic.dir.join("README.md")) {
                let readme = crate::agent::provider::truncate(&readme, 1800);
                p.push_str("\n### 主题 README（用户自己写的背景资料，优先级高于你的猜测）\n");
                p.push_str(&readme);
                p.push('\n');
            }

            // 笔记清单：让模型知道「已经有什么」，避免重复造轮子
            let notes: Vec<String> = store::walk_files(&topic.notes_dir(), 3)
                .into_iter()
                .filter(|p| p.extension().is_some_and(|e| e == "md"))
                .take(40)
                .map(|p| topic.rel(&p))
                .collect();
            if !notes.is_empty() {
                p.push_str("\n### 已有笔记（写新笔记前先看看能不能续写）\n");
                for n in &notes {
                    p.push_str(&format!("- {n}\n"));
                }
            }

            // 资料清单：PDF / Markdown / txt / 图片都可能。深度和「资料」面板一致（4 层），
            // 否则用户放进子目录里的讲义模型看不见，就会答「你的资料里没有」。
            let materials: Vec<String> = store::walk_files(&topic.materials_dir(), 4)
                .into_iter()
                .take(40)
                .map(|p| {
                    let rel = topic.rel(&p);
                    let size = std::fs::metadata(&p).map(|m| crate::paths::human_size(m.len())).unwrap_or_default();
                    format!("{rel}（{size}）")
                })
                .collect();
            if !materials.is_empty() {
                p.push_str(
                    "\n### 主题资料（PDF / Markdown / txt / 图片都有；用 viewer_open 打开、viewer_read 阅读）\n",
                );
                for m in &materials {
                    p.push_str(&format!("- {m}\n"));
                }
            }

            // 用户自己丢进来的讲义（老师划的重点）
            let kb_files: Vec<String> = store::walk_files(&topic.kb_dir(), 4)
                .into_iter()
                .take(40)
                .map(|p| topic.rel(&p))
                .collect();
            if !kb_files.is_empty() {
                p.push_str("\n### 用户上传的讲义与课件（kb/）——这是老师划的重点，优先级最高\n");
                for f in &kb_files {
                    p.push_str(&format!("- {f}\n"));
                }
            }

            // 继承来的资料：父主题（乃至更上层）的 materials/、kb/、notes/。
            // 学「某一章」时这就是老师那份整门课的讲义 + 你以前学过的笔记，属于必看内容。
            if !ancestors.is_empty() {
                if let Some(root) = topic.dir.parent() {
                    let mut listed: Vec<String> = Vec::new();
                    for a in ancestors.iter().take(3) {
                        for sub in [
                            crate::domain::topic::DIR_MATERIALS,
                            crate::domain::topic::DIR_KB,
                            crate::domain::topic::DIR_NOTES,
                        ] {
                            for f in store::walk_files(&a.dir.join(sub), 3).into_iter().take(20) {
                                listed.push(crate::paths::rel_in_root(root, &f));
                            }
                        }
                    }
                    if !listed.is_empty() {
                        p.push_str(
                            "\n### 继承的资料（来自父主题，只读）\n\
                             这些是上级主题的资料、讲义和笔记，本主题可以直接读；\
                             引用时把路径**照原样写全**（含主题名前缀），用户点得动。\
                             不要改或删父主题里的文件——要记东西就写进本主题的 notes/。\n",
                        );
                        for l in listed.iter().take(24) {
                            p.push_str(&format!("- {l}\n"));
                        }
                    }
                }
            }

            // 知识库索引摘要（标题级），让模型知道每份资料大概讲了什么
            let index = crate::kb::load_index(&topic.dir);
            if !index.files.is_empty() {
                p.push_str("\n### 知识库摘要（讲之前先用 kb_search 检索相关段落，并标出处）\n");
                p.push_str(&crate::kb::digest(&index, 24, 5));
            } else {
                p.push_str(
                    "\n### 知识库\n还没建索引。用户给了讲义/资料时，先用 `kb_build` 建一次，之后用 `kb_search` 检索。\n",
                );
            }

            // 进行中的讲解方案：让模型跨轮次也知道自己讲到哪了
            if let Some(plan) = crate::commands::lesson::active_plan(&topic.dir) {
                p.push_str("\n### 进行中的讲解方案（讲完一步记得用 lesson_step 标记）\n");
                p.push_str(&plan.digest());
                p.push_str("\n继续按这份方案推进：只讲当前这一步，讲完提问，等用户回应。\n");
            }

            // 其它主题：预习时要指出「哪些前置知识你已经学过了」
            if let Some(root) = topic.dir.parent() {
                if let Some(others) = other_topics_digest(root, topic) {
                    p.push_str("\n### 用户学过的其它主题（写前置知识、做类比时用得上）\n");
                    p.push_str(&others);
                    p.push_str(
                        "需要细节时，用 `fs_read`/`kb_search` 传 `topic` 参数去那个主题里查，不要凭猜测说「你学过」。\n",
                    );
                }
            }

            // 未完成任务：让 agent 知道「上次说到哪」
            let tasks = crate::domain::task::PlanTask::open_list(topic.tasks_path().as_path());
            if !tasks.is_empty() {
                p.push_str("\n### 未完成的计划\n");
                for t in tasks.iter().take(12) {
                    let due = t
                        .due
                        .map(|d| format!("（截止 {}）", d.format("%m-%d")))
                        .unwrap_or_default();
                    p.push_str(&format!("- [{}] {}{}\n", t.status.label(), t.title, due));
                }
            }

            p.push_str("\n### 阶段要求\n");
            p.push_str(stage.directive());
            p.push('\n');
        }
        None => {
            p.push_str(
                "\n## 当前没有打开主题\n\
                 用户在首页跟你聊天。如果聊着聊着出现了一个明确的学习对象，\
                 主动建议「要不要建一个主题专门追踪它」，并调用 `topic_create` 建好。\n",
            );
        }
    }

    // ---- 用户自定义 ----
    if !inp.config.agent.system_prompt_extra.trim().is_empty() {
        marks.push(("用户要求", p.len()));
        p.push_str("\n## 用户的额外要求（优先级最高）\n");
        p.push_str(inp.config.agent.system_prompt_extra.trim());
        p.push('\n');
    }

    p.push_str(&format!(
        "\n## 称呼\n用户希望被称作「{}」。\n",
        inp.config.user_name
    ));

    // 按各版块的起点切出大小（估算，只给界面画比例用）
    let sections: Vec<PromptSectionStat> = marks
        .iter()
        .enumerate()
        .map(|(i, (label, start))| {
            let end = marks.get(i + 1).map(|(_, s)| *s).unwrap_or(p.len());
            PromptSectionStat { label, tokens: estimate_tokens(&p[*start..end]) }
        })
        .filter(|s| s.tokens > 0)
        .collect();
    (p, sections)
}

// ============================================================ 工坊模式

/// 工坊模式的提示词入参。
///
/// 单独一套而不是复用 [`PromptInputs`]：工坊**没有主题**，
/// 那份输入里的 stage / topic / 资料清单在这里全是空的，硬塞进去只会让两份提示词的
/// 分支互相纠缠（「这里为什么没有主题」在两边含义不一样）。
pub struct StudioInputs<'a> {
    pub config: &'a AppConfig,
    pub supports_tools: bool,
    pub tool_catalog: String,
    pub tool_names: Vec<String>,
    /// 已有的技能清单：造新技能前先看看有没有能续写的
    pub skill_catalog: String,
    /// 练习目录（工作区相对路径，例如 `.hub/workshop`）
    pub bench: String,
    /// 全局长期记忆（工坊里没有记忆工具，但知道用户是谁仍然有用）
    pub memory: String,
}

pub fn build_studio_prompt(inp: &StudioInputs<'_>) -> String {
    build_studio_prompt_with_stats(inp).0
}

/// 工坊模式的系统提示词（各版块大小同样算出来，给「上下文构成」浮层用）。
pub fn build_studio_prompt_with_stats(inp: &StudioInputs<'_>) -> (String, Vec<PromptSectionStat>) {
    use crate::agent::message::estimate_tokens;

    let mut p = String::with_capacity(4096);
    let mut marks: Vec<(&'static str, usize)> = vec![("系统提示词", 0)];

    p.push_str(&format!(
        "你是「学习中枢」里的**工坊助手**。这个模式不学任何具体课程，只做一件事：\
         按用户的要求做出能装进这个应用的**技能（Skill）**与 **MCP 服务器**，并且真的把它们发布出去。\n\
         \n\
         ## 工作方式\n\
         - 用中文回答（用户用其它语言时跟随用户）。输出用 Markdown。\n\
         - **先读规范再动手**：造技能前 `spec_read` 读 `skill`，造 MCP 服务器前读 `mcp`。\n\
         那两份文档写的是本应用**真正实现**的行为（目录约定、协议细节、发布参数怎么填），\n\
         和你记忆里其它 agent 工具的做法不一样——不要凭印象写。\n\
         - **先问清再开工**：一句话需求（「帮我做个查词的」）先落成具体约定：数据从哪来、\n\
         输入什么、输出什么、谁在什么场景用。拿不准的细节一次问清（别挤牙膏），\n\
         能按规范默认做法先做的就做，但要在回复里说明你替用户定了什么。\n\
         - **写完要跑一遍**：技能发布后用 `skill_list` 确认它进了清单；\n\
         MCP 服务器发布后看 `mcp_publish` 返回的连接状态与工具名，再**实际调用一次**那个工具验证行为。\n\
         - **落盘而不是口述**：用 `fs_write` 把文件写全，别只在回复里贴一段代码说「大概这样」。\n\
         - 一次做一步、做完了说清「现在能用了，你可以…」，不要把半成品说成成品。\n\
         \n\
         ## 边界\n\
         - 你的工作目录是**练习目录** `{}`（相对路径都以它为根）：\n\
         技能草稿放 `{}/<id>/`，服务器草稿放 `{}/<id>/`。只有发布之后才会进正式目录。\n\
         - 这里没有主题、没有资料库、没有卡片与计划，也**没有 shell**：不能执行任意命令。\n\
         服务器唯一的验证方式就是 `mcp_publish`——它会真的把进程起起来并握手，\n\
         失败时把 stderr 摘要带回来。别指望用别的方式「先跑一下看看」。\n\
         - 技能与服务器是用户**以后长期用**的东西：宁缺勿滥。一次性的任务不要造技能；\n\
         两三句话能说清的事也不值得。\n\
         - 不要写入练习目录与正式目录以外的位置。想改用户的主题内容，那是学习模式的事，\n\
         提醒他切过去做。\n",
        inp.bench, crate::studio::DRAFT_SKILLS, crate::studio::DRAFT_MCP
    ));

    marks.push(("记忆", p.len()));
    if inp.memory.trim().is_empty() {
        p.push_str("\n## 长期记忆\n现在还没有关于这位用户的长期记忆。\n");
    } else {
        p.push_str("\n## 长期记忆（以前记下的；这里只读，不能新增）\n");
        p.push_str(inp.memory.trim());
        p.push('\n');
        p.push_str("上面这些说明用户是谁、习惯怎样——做工具时照顾到（例如他习惯中文界面、用 Windows）。\n");
    }

    if !inp.skill_catalog.trim().is_empty() {
        marks.push(("技能", p.len()));
        p.push_str("\n## 用户已有的技能（造新技能前先看这里，能续写就别新建）\n");
        p.push_str(&inp.skill_catalog);
    }

    marks.push(("工具说明", p.len()));
    if inp.supports_tools {
        p.push_str(&format!(
            "\n## 可用工具\n用原生 function calling 调用下列工具，不要用文字假装调用：\n{}\n\
             只能用上面列出的工具名。\n",
            inp.tool_catalog
        ));
    } else {
        p.push_str(&format!(
            "\n## 可用工具（当前模型未开启原生工具调用）\n\
             把想做的操作写成如下格式放在回复末尾，由用户确认：\n\
             ```tool\n{{\"name\": \"工具名\", \"input\": {{...}}}}\n```\n\
             可用工具：\n{}\n",
            inp.tool_catalog
        ));
    }

    marks.push(("工坊", p.len()));
    p.push_str(&format!(
        "\n## 发布流程（照这个顺序做）\n\
         1. `spec_read` 读对应规范（skill / mcp）。\n\
         2. 用 `fs_list` 看一眼练习目录里已有什么，`skill_list` 看已有技能，`mcp_status` 看已登记的服务器。\n\
         3. 用 `fs_write` 把文件写全（技能必须有 SKILL.md；服务器要有源码与 README）。\n\
         4. `skill_publish(dir)` 或 `mcp_publish(dir, name, command, args, env)` 发布。\n\
         5. 验证：技能看 `skill_list`，服务器看返回的状态并调用它暴露的工具。\n\
         6. 有错就改文件再发布一次——发布是覆盖式的，不必先删。\n\
         \n\
         用户可以在侧栏的「技能」与「MCP 服务器」面板里查看、开关、删除这些东西；\n\
         工坊练习目录的位置是 `<工作区>/{}/`（相对路径就是相对它）。\n",
        inp.bench
    ));

    // ---- 用户自定义（和学模式一样，最高优先级） ----
    if !inp.config.agent.system_prompt_extra.trim().is_empty() {
        marks.push(("用户要求", p.len()));
        p.push_str("\n## 用户的额外要求（优先级最高）\n");
        p.push_str(inp.config.agent.system_prompt_extra.trim());
        p.push('\n');
    }

    p.push_str(&format!(
        "\n## 称呼\n用户希望被称作「{}」。\n",
        inp.config.user_name
    ));

    let sections: Vec<PromptSectionStat> = marks
        .iter()
        .enumerate()
        .map(|(i, (label, start))| {
            let end = marks.get(i + 1).map(|(_, s)| *s).unwrap_or(p.len());
            PromptSectionStat { label, tokens: estimate_tokens(&p[*start..end]) }
        })
        .filter(|s| s.tokens > 0)
        .collect();
    (p, sections)
}

pub fn current_date_line() -> String {
    format!("今天是 {}。", store::now().format("%Y-%m-%d %A"))
}

/// 把「用户还学过什么」压缩成一小段：主题名 + 阶段 + 笔记标题 + 卡片数。
///
/// 预习时要回答「这个东西需要哪些前置、我学过没有」，靠的就是这段。
/// 只给标题级信息（不给正文），细节让模型自己用 kb_search / fs_read 去取。
/// 本主题的父/子主题照样列（它们通常最相关），但标注清楚关系，
/// 免得模型把「同一门课的另一章」当成两门不相干的课。
fn other_topics_digest(root: &std::path::Path, current: &Topic) -> Option<String> {
    let ws = crate::domain::topic::Workspace::new(root.to_path_buf());
    let mut out = String::new();
    for summary in ws.list().ok()?.into_iter().take(12) {
        if summary.slug == current.slug() {
            continue;
        }
        // 卡片数为 0 且没有笔记的主题多半只是刚建的空壳，不值得列
        if summary.stats.notes == 0 && summary.stats.cards == 0 {
            continue;
        }
        let relation = if summary.meta.parent.as_deref() == Some(current.meta.id.as_str()) {
            "（本主题的子主题）"
        } else if current.meta.parent.as_deref() == Some(summary.meta.id.as_str()) {
            "（本主题的父主题）"
        } else {
            ""
        };
        let Ok(topic) = ws.load(&summary.slug) else { continue };
        let notes: Vec<String> = store::walk_files(&topic.notes_dir(), 3)
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
            .take(6)
            .map(|p| {
                std::path::Path::new(&topic.rel(&p))
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default()
            })
            .filter(|s| !s.is_empty())
            .collect();
        out.push_str(&format!(
            "- 「{}」{relation}（{}阶段，{} 张卡片）",
            summary.meta.name,
            summary.meta.stage.label(),
            summary.stats.cards
        ));
        if !notes.is_empty() {
            out.push_str(&format!("：笔记有 {}", notes.join("、")));
        }
        if !summary.meta.description.trim().is_empty() {
            out.push_str(&format!("｜简介：{}", summary.meta.description.trim()));
        }
        out.push('\n');
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::topic::Workspace;

    /// 子主题（「一门课的一章」）的系统提示词：
    /// 要写明上级主题、把父主题的资料列成「继承的资料」并带主题名前缀（这样引用可点击），
    /// 还要在其它主题清单里标明父子关系，免得模型当成两门不相干的课。
    #[test]
    fn prompt_includes_inherited_materials_for_subtopic() {
        let tmp = std::env::temp_dir().join(format!("lh-prompt-{}", uuid::Uuid::new_v4()));
        let ws = Workspace::new(&tmp);
        let parent = ws.create("线性代数", "整门课", None, None).unwrap();
        std::fs::write(parent.materials_dir().join("lecture1.md"), "# 特征值\n定义：$Av=\\lambda v$").unwrap();
        std::fs::write(parent.notes_dir().join("第一讲.md"), "# 第一讲\n").unwrap();
        std::fs::write(parent.dir.join("README.md"), "整门课的说明").unwrap();
        let child = ws.create("第三章 特征值", "只学这章", None, Some("线性代数")).unwrap();

        let cfg = AppConfig::bootstrap(tmp.clone());
        let prompt = build_system_prompt(&PromptInputs {
            config: &cfg,
            topic: Some(&child),
            stage: None,
            supports_tools: true,
            tool_catalog: String::new(),
            tool_names: Vec::new(),
            skill_catalog: String::new(),
            memory: String::new(),
        });

        assert!(prompt.contains("- 上级主题：线性代数"), "缺少上级主题行");
        assert!(prompt.contains("### 继承的资料（来自父主题，只读）"), "缺少继承资料段");
        assert!(
            prompt.contains("线性代数/materials/lecture1.md"),
            "继承资料的引用路径必须带主题名前缀"
        );
        assert!(prompt.contains("线性代数/notes/第一讲.md"), "父主题的笔记也该列出来");
        // 父主题在「其它主题」清单里要被标注成父主题
        assert!(prompt.contains("（本主题的父主题）"), "其它主题清单没标出父子关系");
        // 父主题自己的资料不该被当成「本主题」的资料重复列一遍
        assert!(!prompt.contains("### 主题资料"), "本主题没有资料，不该出现资料清单");

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 长期记忆要出现在系统提示词里，而且位置在「当前主题」之前——
    /// 讲什么之前先知道该对谁讲。没有记忆时也要给一段「什么时候该记」的指引。
    #[test]
    fn prompt_carries_memory_block_before_topic() {
        let tmp = std::env::temp_dir().join(format!("lh-prompt-mem-{}", uuid::Uuid::new_v4()));
        let ws = Workspace::new(&tmp);
        let topic = ws.create("线性代数", "整门课", None, None).unwrap();
        let cfg = AppConfig::bootstrap(tmp.clone());

        let with_mem = build_system_prompt(&PromptInputs {
            config: &cfg,
            topic: Some(&topic),
            stage: None,
            supports_tools: true,
            tool_catalog: String::new(),
            tool_names: Vec::new(),
            skill_catalog: String::new(),
            memory: "**注意**\n- ★他把特征值和特征向量搞混（2024-05-01）\n".to_string(),
        });
        let mem_at = with_mem.find("## 长期记忆").expect("缺少长期记忆段");
        let topic_at = with_mem.find("## 当前主题").expect("缺少当前主题段");
        assert!(mem_at < topic_at, "长期记忆应排在当前主题之前");
        assert!(with_mem.contains("他把特征值和特征向量搞混"));
        // 提醒模型别重复记、并且要判断时效
        assert!(with_mem.contains("别再重复记"));
        assert!(with_mem.contains("memory_write"));

        let empty = build_system_prompt(&PromptInputs {
            config: &cfg,
            topic: Some(&topic),
            stage: None,
            supports_tools: true,
            tool_catalog: String::new(),
            tool_names: Vec::new(),
            skill_catalog: String::new(),
            memory: String::new(),
        });
        assert!(empty.contains("还没有关于这位用户的长期记忆"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 「上下文构成」浮层靠这个：各版块要分开统计，且加起来不该超过整段提示词
    /// （按字符估会有取整误差，所以留一点余量）。
    #[test]
    fn prompt_stats_split_each_section() {
        let tmp = std::env::temp_dir().join(format!("lh-prompt-stats-{}", uuid::Uuid::new_v4()));
        let ws = Workspace::new(&tmp);
        let topic = ws.create("线性代数", "整门课", None, None).unwrap();
        let cfg = AppConfig::bootstrap(tmp.clone());

        let (prompt, sections) = build_system_prompt_with_stats(&PromptInputs {
            config: &cfg,
            topic: Some(&topic),
            stage: None,
            supports_tools: true,
            tool_catalog: "- fs_read：读文件\n".to_string(),
            tool_names: vec!["fs_read".to_string()],
            skill_catalog: "- blender：三维建模\n".to_string(),
            memory: "**情况**\n- 他在准备期末考试\n".to_string(),
        });

        let labels: Vec<&str> = sections.iter().map(|s| s.label).collect();
        for want in ["系统提示词", "记忆", "技能", "工具说明", "主题与资料"] {
            assert!(labels.contains(&want), "缺少版块：{want}（实际 {labels:?}）");
        }

        let sum: u32 = sections.iter().map(|s| s.tokens).sum();
        let whole = crate::agent::message::estimate_tokens(&prompt);
        assert!(
            sum <= whole + sections.len() as u32,
            "分块求和 {sum} 不该明显超过整段 {whole}"
        );

        std::fs::remove_dir_all(&tmp).ok();
    }
}
