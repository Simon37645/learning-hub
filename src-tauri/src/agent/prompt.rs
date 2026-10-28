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
         - **要动起来就用 `page_create` 建演示页**：它写一个**真的会跑 JavaScript 的本地网页**，\
         并在内置浏览器里打开。三维结构（分子、晶体、器官、装置）、波与振动、受力与轨迹、\
         参数怎么影响结果、算法逐步执行——这些「光靠文字讲不清」的东西就该用页面：\
         能 import 内置的 three.js（离线可用）、能做动画、能加滑块和按钮让用户自己拖着看。\
         页面要自带说明（这是什么、怎么操作）、深浅色都看得清、画布跟随窗口大小、\
         给一个能让用户自己转/orbit 的交互。**写完必须自查**：`web_eval` 跑几行 JS 确认关键\
         对象真建出来了（把 scene / renderer 挂到 window 上才查得到）、`web_screenshot` 看一眼\
         画面是不是空白或全黑；不对就改，再调一次 `page_create`（同一个 id 覆盖重写并自动刷新）。\
         一句话：**能动的、能转的、要用户拖的，都做页面**；只在需要静态单页时才手写 \
         `lessons/<名字>.html` 用 `viewer_open` 打开（那条路不跑脚本）。\n\
         - **讲不清的就直接画在回复里**：动态过程、空间关系、参数怎么影响结果、抽象结构这类\
         「看一秒胜过讲十句」的**小**内容，在回复里直接放一个 ```html 代码块——应用会把它渲染成\
         对话里的**演示卡片**，用户不用点开任何东西就能看到、能跟着动。\
         卡片是沙箱 iframe，**里面的 JavaScript 不会执行**：动画用 CSS（animation / transition /\
         @keyframes）或 SVG 的 <animate>、<animateTransform>，交互用 :hover / :checked /\
         <details> 这类纯 CSS 手段；不要写 <script>，也别指望 fetch。\
         静态的示意图、几何图形、坐标轴用 ```svg 代码块（同样会渲染成卡片）。\
         写演示的几条硬要求：自包含（内联样式、不引外部资源）、自带配色（深浅色主题下都要看得清）、\
         高度按 360px 左右设计、别依赖滚动才能看懂。\
         判断标准：讲完这一段，用户脑子里得「看见」某个东西才明白——那就给演示，\
         而不是再堆一段文字。**要 JS / 三维 / 真的交互 → 用 page_create 建页面；\
         纯 CSS 的小示意 → 用卡片**（卡片轻，用户不用点开）。\n\
         - **先读字，再读图**：PDF 的文字层里没有插图、示意图和公式图。正文里出现「如图」「下表」\
         「(a)(b)」这类指代、或者读出来明显不成句（扫描版），就用 `pdf_screenshot` 把那一页\
         渲染成图片看一眼再回答——这是对文档那一页截图，不受窗口遮挡影响。\
         看图时以图为准：单位、坐标轴、公式的写法按图里画的来，文字层里对不上号的地方以图为准。\n\
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
         不要写「第三章」这类章节名当页码（只有确实需要指章节时才写，那会退化成在文档里搜这段字）。\n\
         - **拿不准就查，别硬编**：工具表里如果有检索类工具（内置的 `web_search`，或者 MCP 服务器\
         提供的 `mcp__…__search` 之类——包括用户自己配的 Tavily 这类），下面这些情况**先搜再答**：\
         近几年的进展与最新结论、指南/标准/剂量/剂量单位这类**要精确**的数字、谁在什么时候提出、\
         要找具体文献与出处、以及你自己心里也没底的事实。\
         搜索与抓取**按次计费**：一次搜索能给出一组带摘要的结果，先用摘要筛；\
         确实要细节才对具体 url 用 `web_fetch` 读一两篇把细节坐实（别拿标题和摘要下结论），\
         回答时说清来源（标题 + 链接），并且把「查到的」和「我的推断」分开写。\
         反过来：教材级的稳定知识不需要每次去搜，用户自己的讲义仍然优先于网上的东西；\
         讲义与检索结果冲突时，两个都告诉他，别偷偷选一个。\
         **没有联网工具、或者搜不到，就直说不知道**——绝不能用记忆里的数字冒充检索结果，\
         更不能编文献名与页码。
\n\
         ## 文件约定\n\
         - 所有路径都是**相对当前主题目录**的相对路径，例如 `notes/特征值.md`、`materials/lecture1.pdf`。\n\
         - 笔记放 `notes/`，资料放 `materials/`，讲义放 `kb/`，讲解方案与静态讲解页放 `lessons/`；\
         可交互的演示页由 `page_create` 写到 `.hub/pages/<名字>/`（一个页面可以是**一整个目录**：\
         入口 index.html + 它自己的脚本与数据）。\n\
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
         - **用户多半不懂技术**：他只会用大白话说想要什么。别把「你去装个包」「你自己跑一下看报错」\n\
         这类话丢给他——能自己做的就自己做完（跑脚本、装依赖、试、看报错、改），最后直接告诉他结论。\n\
         只有你替不了他的事才问：数据从哪来、给谁用、结果要长什么样。\n\
         - **先读规范再动手**：造技能前 `spec_read` 读 `skill`，造 MCP 服务器前读 `mcp`。\n\
         那两份文档写的是本应用**真正实现**的行为（目录约定、协议细节、发布参数怎么填），\n\
         和你记忆里其它 agent 工具的做法不一样——不要凭印象写。\n\
         - **先问清再开工**：一句话需求（「帮我做个查词的」）先落成具体约定：数据从哪来、\n\
         输入什么、输出什么、谁在什么场景用。拿不准的细节一次问清（别挤牙膏），\n\
         能按规范默认做法先做的就做，但要在回复里说明你替用户定了什么。\n\
         - **自己验证到底**：服务器先用 `mcp_probe` 试运行、必要时用 `run_command` 跑一遍看报错，\n\
         发布之后再**实际调用一次**它暴露的工具；技能用 `skill_list` 确认进了清单。\n\
         出错就改、改完再验——闭环到「真的能用」为止，别把「应该能行」当结论。\n\
         - **落盘而不是口述**：用 `fs_write` 把文件写全，别只在回复里贴一段代码说「大概这样」。\n\
         - 一次做一步、做完了说清「现在能用了，你可以…」，不要把半成品说成成品。\n\
         \n\
         ## 边界\n\
         - 你的工作目录是**练习目录** `{}`（相对路径都以它为根）：\n\
         技能草稿放 `{}/<id>/`，服务器草稿放 `{}/<id>/`。只有发布之后才会进正式目录。\n\
         - 练习目录里可以用 `run_command` 跑命令（Windows 上是 `cmd.exe /C <一行命令>`）——\n\
         装依赖、跑脚本、看报错都靠它。几条硬约束：一次一条命令；**不能交互**（没有 stdin，\n\
         要确认的安装加 `-y` / `--yes`）；没有管理员权限；有超时（默认 60 秒，最多 600）。\n\
         长任务拆开跑、把输出重定向到文件再看。**别装系统级的东西**（全局 npm 包、系统软件、\n\
         改注册表或 PATH）；依赖优先装进服务器自己的目录（`pip install --target .`、\n\
         `npm install --prefix .`），确实非装全局不可就先问用户。能不用依赖就不用：\n\
         标准库够用的方案最省事，也不会在用户机器上装不上。\n\
         - 这里没有主题、资料库、卡片与计划——那些是学习模式的东西，工坊里也拿不到对应的工具。\n\
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
         4. 服务器先 `mcp_probe` 试运行一次：它会把进程真起起来、握手、列工具再退出，\n\
         起不来时直接把 stderr 与退出码给你（比发布失败后再猜快得多）。脚本本身有问题就用\n\
         `run_command` 直接跑一遍看报错。\n\
         5. `skill_publish(dir)` 或 `mcp_publish(dir, name, command, args, env)` 发布。\n\
         6. 验证：技能看 `skill_list`，服务器看返回的连接状态与工具名，并调用它暴露的工具。\n\
         7. 有错就改文件再发布一次——发布是覆盖式的，不必先删。\n\
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

// ============================================================ 深度研究

/// 深度研究轮可用的检索通道。协议按它裁剪：哪边可用就教用哪边，
/// 哪边不可用就明说「本轮用不了」并给降级路子——不让模型对着不存在的工具空转，
/// 也不让它把「没得查」当成「不用标来源」的理由。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ResearchTools {
    /// 联网搜索可用（联网总闸 + DeepSeek 配置都就绪：web_search / web_fetch 能用）
    pub web_search: bool,
    /// 本地知识库可用（学习模式且开着主题：kb_search 存在，索引没建会自动建）
    pub kb_search: bool,
}

/// 深度研究协议：追加到系统提示词末尾（见 `run_turn` 的 `deep_research` 分支）。
///
/// 流程借鉴三个 MIT 协议的开源实现（**只借思路，没有引代码**）：
/// dzhng/deep-research 的「查询 → 阅读 → 反思缺口 → 下一轮查询」反馈式循环、
/// GPT Researcher 的「计划 → 分头检索 → 汇总成报告」结构、
/// stanford-oval/storm 的「多视角子问题 + 引用先行」写作要求。
pub fn deep_research_protocol(tools: ResearchTools) -> String {
    // 检索通道指引：按可用性四选一
    let channels = match (tools.web_search, tools.kb_search) {
        (true, true) => "\
             - **用户的讲义与笔记**：`kb_search`（带页码出处，**优先查**——老师划过重点的内容比网上的可靠；\
             来源冲突时两边都写并指出差异）。\
             - **联网搜索**：`web_search`（按次计费：先读摘要筛，确实要细节才对具体 url 用 `web_fetch` 读原文）；\
             内置浏览器里已经打开的页面用 `viewer_list` / `viewer_read` 直接读，不用重复抓取。\
             两条腿都用：先讲义后网络，缺口靠网络补。",
        (true, false) => "\
             - **联网搜索**：`web_search`（按次计费：先读摘要筛，确实要细节才对具体 url 用 `web_fetch` 读原文）；\
             内置浏览器里已经打开的页面用 `viewer_list` / `viewer_read` 直接读。\
             本轮没有可检索的本地知识库，结论以网上来源为主。",
        (false, true) => "\
             - **用户的讲义与笔记**：`kb_search`（带页码出处）。\
             本轮联网搜索不可用：报告要写明「基于本地资料」，查不到的部分**如实列入「未尽事项」**，\
             不要用记忆里的内容冒充检索结果。",
        (false, false) => "\
             - **本轮没有任何检索通道**（没配联网搜索，也没有可检索的本地知识库）。\
             在报告开头明确告诉用户这一点，然后仍按下面的结构给出基于模型已有知识的报告：\
             每条结论都标「（未检索到来源，模型知识）」，把需要核实的问题列进「未尽事项」，\
             **绝不编造来源、页码或链接**。",
    };

    format!(
        "\
## 深度研究模式（仅本轮生效）

用户提出的是研究型问题：这一轮要跑一次完整的小型研究，最后交付一份**带来源的结构化报告**。\
普通的讲课节奏（一次一小步、讲一段停下来提问）**本轮不适用**：中途不要反问用户、不要等确认，\
自己把研究循环跑完（用户随时可以点「停止」）。普通轮的「宁可少写」在这里也不适用——报告要完整，\
但每一句都要有出处或标注不确定性。

### 研究流程

1. **先列研究计划**：把问题拆成 3-6 个具体的子问题，从不同视角审视（现状与背景、关键数据与事实、\
不同立场与争议、与用户讲义/已学内容的关系、时效性）。计划用一小段告诉用户，然后**立刻开查**，不要等确认。
2. **逐个子问题检索与阅读**，边查边把要点连同来源简短记下来（放在工具调用之间的文字里）：
{channels}
3. **每轮反思补漏**：哪些子问题已有着落、哪些还没有、出现了什么值得追的新线索——下一轮就查缺口，\
不重复已确认过的内容。子问题基本收束（或连续两轮没有新进展）就停下写报告。
4. **产出报告**（这是本轮唯一要交付的东西）：
   - **结论先行**：开头给一段「核心结论」，直接回答用户的问题（3-5 条，一条一句）；
   - **分节论述**：按子问题分 ### 小节，把证据与推理讲清楚，关键结论句尾带来源；
   - **区分共识与争议**：多方一致的就说一致；有分歧、数据打架、来源互相矛盾的要明说；
   - **讲义优先**：用户资料与网上说法冲突时，两个都写并指出差异，别偷偷选一个；
   - 结尾给「**参考来源**」清单（每条：标题/文件 + 位置）和「**未尽事项**」（没查到、不确定的，直说）。

### 来源格式（用户要点得动）

- 网页：写 Markdown 链接，句尾如「……（[标题](https://…)）」，文末「参考来源」同格式；
- 本地资料：句尾写 【来源：文件相对路径 第 N 页】（页码**照抄 kb_search 给的**，不要自己编；
  查不到页码只写路径）；
- 没查到来源、只凭模型自己知识的内容，明确标注「（未检索到来源，模型知识）」\
——绝不能用记忆里的数字或文献冒充检索结果。
",
        channels = channels
    )
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

    /// 「讲不清就画在回复里」这条约定必须留在提示词里，并且要说清**沙箱不跑脚本**——
    /// 少了这句，模型会写一堆 <script>，用户在卡片里只看到一张不动的图。
    #[test]
    fn prompt_teaches_inline_demo_and_pdf_figures() {
        let tmp = std::env::temp_dir().join(format!("lh-prompt-demo-{}", uuid::Uuid::new_v4()));
        let ws = Workspace::new(&tmp);
        let topic = ws.create("线性代数", "整门课", None, None).unwrap();
        let cfg = AppConfig::bootstrap(tmp.clone());
        let prompt = build_system_prompt(&PromptInputs {
            config: &cfg,
            topic: Some(&topic),
            stage: None,
            supports_tools: true,
            tool_catalog: String::new(),
            tool_names: Vec::new(),
            skill_catalog: String::new(),
            memory: String::new(),
        });

        assert!(prompt.contains("```html 代码块"), "缺少内联演示的约定");
        assert!(prompt.contains("不会执行"), "必须说明演示卡片里不跑 JavaScript");
        assert!(prompt.contains("```svg"), "静态图要用 svg 代码块");
        assert!(prompt.contains("pdf_screenshot"), "看图要用 pdf_screenshot");
        assert!(prompt.contains("以图为准"), "要说明图文冲突时以图为准");
        // 会跑的演示页：三维/动画/可交互走 page_create，而且必须让模型知道「写完要自查」
        assert!(prompt.contains("page_create"), "缺少会跑的演示页约定");
        assert!(prompt.contains("three.js"), "要写明内置的 three.js 可用");
        assert!(prompt.contains("web_screenshot"), "要教它自查画面（截图看一眼）");
        // 联网检索：拿不准就查，查不到就说不知道
        assert!(prompt.contains("web_search"), "缺少联网检索的约定");
        assert!(prompt.contains("绝不能用记忆里的数字冒充检索结果"), "要禁止编造检索结果");

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 深度研究协议：完整流程的骨架必须在；按可用通道裁剪——
    /// 只剩一边时另一边要明说不可用，两边都没有时要有中文降级说明且禁止编造来源。
    #[test]
    fn deep_research_protocol_adapts_to_available_tools() {
        let both = deep_research_protocol(ResearchTools { web_search: true, kb_search: true });
        for want in [
            "研究计划",
            "反思补漏",
            "核心结论",
            "参考来源",
            "未尽事项",
            "kb_search",
            "web_search",
            "web_fetch",
            "【来源：文件相对路径 第 N 页】",
            "（[标题](https://…)）",
        ] {
            assert!(both.contains(want), "两侧可用时缺少「{want}」");
        }

        // 只剩联网：要说明本地知识库不可用，且不再教 kb_search
        let web_only = deep_research_protocol(ResearchTools { web_search: true, kb_search: false });
        assert!(web_only.contains("没有可检索的本地知识库"), "要明说本地知识库不可用");
        assert!(!web_only.contains("`kb_search`"), "不该教模型用不存在的工具：{web_only}");

        // 只剩知识库：要说明联网不可用，且缺口进「未尽事项」
        let kb_only = deep_research_protocol(ResearchTools { web_search: false, kb_search: true });
        assert!(kb_only.contains("联网搜索不可用"), "{kb_only}");
        assert!(!kb_only.contains("`web_search`"), "不该教模型用不存在的工具：{kb_only}");
        assert!(kb_only.contains("基于本地资料"));

        // 两侧都不可用：中文降级说明 + 禁止编造来源
        let none = deep_research_protocol(ResearchTools::default());
        assert!(none.contains("没有任何检索通道"), "{none}");
        assert!(none.contains("未检索到来源，模型知识"));
        assert!(none.contains("绝不编造来源"));
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
