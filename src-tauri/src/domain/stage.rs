//! 学习阶段：预习 → 学习 → 复习 → 测验。
//!
//! 这是「学习中枢」区别于普通聊天工具的主线：每个主题有自己的当前阶段，
//! agent 的系统提示词会随阶段切换（预习给框架、学习给讲解、复习给追问、测验给出题）。

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum StudyStage {
    /// 预习：先建地图，不求甚解
    #[default]
    Preview,
    /// 学习：逐点吃透
    Learn,
    /// 复习：间隔重复 + 主动回忆
    Review,
    /// 测验：出题检验掌握度
    Test,
}

impl StudyStage {
    pub const ALL: [StudyStage; 4] = [
        StudyStage::Preview,
        StudyStage::Learn,
        StudyStage::Review,
        StudyStage::Test,
    ];

    pub fn label(self) -> &'static str {
        match self {
            StudyStage::Preview => "预习",
            StudyStage::Learn => "学习",
            StudyStage::Review => "复习",
            StudyStage::Test => "测验",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            StudyStage::Preview => "preview",
            StudyStage::Learn => "learn",
            StudyStage::Review => "review",
            StudyStage::Test => "test",
        }
    }

    /// 给 agent 的阶段行为约定。
    pub fn directive(self) -> &'static str {
        match self {
            StudyStage::Preview => {
                "当前处于【预习】阶段，走**讲解模式**。目标是「搭地图 + 接上前置」，不求甚解但求位置清楚：\n\
                 1. 先用 `kb_search` 翻用户的讲义，弄清这次要学的范围里老师划了哪些重点；\n\
                 2. 用 `lesson_plan` 出一份讲解方案，写明：这次要讲的 4~8 个模块、每个模块的难点、\
                 每个模块用什么问题检验听懂了；同时用 `mindmap_create` 画一张框架导图，\
                 让用户先看到「这门东西长什么样、各块怎么连」（图比一段文字更适合当地图用）；\n\
                 3. 在方案的 prereqs 里**明确指出前置知识**，并说明「这个前置你在「某主题」里已经学过 / 还没学过」——\
                 已经学过的要接一句「就是那里的哪个结论」，没学过的要标出「需要先补」；\n\
                 4. 然后按方案一步步讲，一次只推进一个模块，讲完就提出 check 问题等用户回答。\n\
                 不要一次倒完所有内容；不要展开细节推导（那是【学习】阶段的事）。"
            }
            StudyStage::Learn => {
                "当前处于【学习】阶段，走**讲解模式**。目标是逐个吃透：\n\
                 一次只处理一个子话题，先给直觉/类比，再给严格定义，再给一个最小例子，最后指出常见误解。\n\
                 用户如果已经上过课（`kb/` 或 `materials/` 里有讲义），先按讲义的重点讲，\
                 再补充讲义里没讲透的地方，并明确指出哪些是「讲义强调的」、哪些是你的补充。\n\
                 讲完主动问用户是否继续，不要连续输出多个子话题。"
            }
            StudyStage::Review => {
                "当前处于【复习】阶段：目标是主动回忆。\n\
                 优先提问而不是讲解：先让用户复述，再指出遗漏与错误，然后补一个变式问题。\n\
                 涉及需要长期记忆的事实，调用 card_create 存成卡片（基础/反向/完形按内容选合适的）。\n\
                 用户答错的点，用 kb_search 找到讲义原文，标出来源一起看。"
            }
            StudyStage::Test => {
                "当前处于【测验】阶段，走**出题模式**。目标是检验掌握度：\n\
                 用 `quiz_create` 出一份小测（5~10 题），题型要搭配：\n\
                 - A1 考单一知识点，A2 给一个场景/病例让用户推理，\n\
                 - B 型考一组容易混淆的概念（共用选项），X 型考「哪些说法正确」（多选，考细节漏洞），\n\
                 - 再配 1~2 道名词解释或简答题（必须给采分点，判分靠它）。\n\
                 出完卷让用户去「工作台 → 测验」作答，不要在这里念答案；\n\
                 用户交卷后再逐题讲解错在哪、该补什么。"
            }
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "preview" | "预习" => Some(StudyStage::Preview),
            "learn" | "学习" => Some(StudyStage::Learn),
            "review" | "复习" => Some(StudyStage::Review),
            "test" | "测验" | "考试" => Some(StudyStage::Test),
            _ => None,
        }
    }
}
