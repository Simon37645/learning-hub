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
}

pub fn build_system_prompt(inp: &PromptInputs<'_>) -> String {
    let mut p = String::with_capacity(4096);

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
         - **该画图就画图**：讲框架、分类、层级关系时用 `mindmap_create` 出思维导图（它会存成笔记并在内置浏览器打开，\
         同时把图定义返回给你，贴进回复用户就能在对话里看到）；讲流程、因果、时间线时，\
         直接把 mermaid 代码块（flowchart / sequenceDiagram / timeline）写进笔记或回复里。\
         图不是装饰：画完要按图上的分支一条条讲，让用户把图当成地图用。\n\
         预习阶段给全局地图时，思维导图几乎总是比一段文字更好用。\n\
         - **引用要标出处**：凡是来自资料的具体内容，都在句末标 `【来源：材料相对路径 第 N 页】`\
         （Markdown 笔记写 `【来源：notes/xx.md】`）。用户会把它变成可点击的跳转，所以路径要写对。\n\n\
         ## 文件约定\n\
         - 所有路径都是**相对当前主题目录**的相对路径，例如 `notes/特征值.md`、`materials/lecture1.pdf`。\n\
         - 笔记放 `notes/`，资料放 `materials/`，讲义放 `kb/`，讲解页与方案放 `lessons/`。\n\
         - 想读写**其它主题**的文件，传 `topic` 参数指定主题名；想访问工作区之外的路径，会先向你申请授权。\n\
         - 写笔记时用 Markdown，标题用 `#`，公式用 LaTeX；文件名用「内容主题」命名，不要用日期堆砌。\n\
         - 覆盖已有文件前先读一遍，确认不会丢掉用户自己写的内容。\n",
    );

    // 技能走渐进式披露：这里只给「有什么技能、什么时候用」
    if !inp.skill_catalog.trim().is_empty() {
        p.push_str("\n## 可用技能（用 skill_list 查看，需要时先 skill_read 读正文再照做）\n");
        p.push_str(&inp.skill_catalog);
        p.push_str(
            "遇到与上面描述相符的任务，先 skill_read 把步骤读完再动手；不要凭印象编技能里的内容。\n",
        );
    }

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

            let materials: Vec<String> = store::walk_files(&topic.materials_dir(), 2)
                .into_iter()
                .take(40)
                .map(|p| {
                    let rel = topic.rel(&p);
                    let size = std::fs::metadata(&p).map(|m| crate::paths::human_size(m.len())).unwrap_or_default();
                    format!("{rel}（{size}）")
                })
                .collect();
            if !materials.is_empty() {
                p.push_str("\n### 主题资料（可以用 viewer_open 打开、用 viewer_read 阅读）\n");
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
                if let Some(others) = other_topics_digest(root, &topic.slug()) {
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
        p.push_str("\n## 用户的额外要求（优先级最高）\n");
        p.push_str(inp.config.agent.system_prompt_extra.trim());
        p.push('\n');
    }

    p.push_str(&format!(
        "\n## 称呼\n用户希望被称作「{}」。\n",
        inp.config.user_name
    ));

    p
}

pub fn current_date_line() -> String {
    format!("今天是 {}。", store::now().format("%Y-%m-%d %A"))
}

/// 把「用户还学过什么」压缩成一小段：主题名 + 阶段 + 笔记标题 + 卡片数。
///
/// 预习时要回答「这个东西需要哪些前置、我学过没有」，靠的就是这段。
/// 只给标题级信息（不给正文），细节让模型自己用 kb_search / fs_read 去取。
fn other_topics_digest(root: &std::path::Path, current_slug: &str) -> Option<String> {
    let ws = crate::domain::topic::Workspace::new(root.to_path_buf());
    let mut out = String::new();
    for summary in ws.list().ok()?.into_iter().take(12) {
        if summary.slug == current_slug {
            continue;
        }
        // 卡片数为 0 且没有笔记的主题多半只是刚建的空壳，不值得列
        if summary.stats.notes == 0 && summary.stats.cards == 0 {
            continue;
        }
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
            "- 「{}」（{}阶段，{} 张卡片）",
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
