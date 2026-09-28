//! 讲解方案工具：把「怎么讲」先定下来，再一步步讲。
//!
//! 预习模式下这两个工具是主干：先 `lesson_plan` 出方案（含前置知识与每一步的检验方式），
//! 每讲完一步调 `lesson_step` 标记。进度会注入系统提示词，跨轮对话也不会迷路。

use crate::agent::registry::{
    arg_str, arg_str_req, arg_u32, arg_str_array, num_prop, object_schema, str_array_prop,
    str_prop, Tool, ToolCtx, ToolOutput,
};
use crate::commands::lesson::{save_plan_inner, LessonInput, LessonPlan, LessonStep, StepStatus};
use crate::error::{AppError, AppResult};
use crate::store;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct LessonPlanTool;

#[async_trait]
impl Tool for LessonPlanTool {
    fn name(&self) -> &'static str {
        "lesson_plan"
    }

    fn description(&self) -> &'static str {
        "先把「怎么讲」定成一份方案，再开始讲。方案常驻系统提示词，跨轮次不会迷路。\n\
         预习阶段必须先用它；学习阶段遇到新章节也建议用。\n\
         steps 建议 4~8 步，每步写清：讲什么（focus）、怎么检验听懂了（check）、用到哪些资料（sources，写清页码）。\n\
         写 prereqs 前如果不知道用户学过什么，先用 kb_search 或 topic_info 查一下。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "id": str_prop("已有方案的 id（改方案时传），新建时留空"),
                "title": str_prop("这次讲解的标题，例如「特征值：从几何直觉到计算」"),
                "goal": str_prop("一句话目标：讲完之后用户应该能做到什么"),
                "prereqs": str_array_prop("前置知识，尽量写成「XX（在「某主题」里学过）」"),
                "steps": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "title": str_prop("这一步讲什么，一句话"),
                            "focus": str_prop("讲解要点"),
                            "check": str_prop("怎么判断用户懂了：要问的问题或要让用户做的事"),
                            "sources": str_array_prop("用到的资料，如 materials/ch1.pdf 第 12 页"),
                            "demo_html": str_prop("如果这一步配了 HTML 演示页，写它的相对路径"),
                        },
                        "required": ["title"],
                    },
                },
            }),
            &["title", "steps"],
        )
    }

    fn risk(&self) -> crate::agent::event::Risk {
        crate::agent::event::Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        let n = input
            .get("steps")
            .and_then(|s| s.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        format!(
            "讲解方案「{}」（{n} 步）",
            arg_str(input, "title").unwrap_or_default()
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let core = ctx.core.clone();
        let topic = core.workspace().resolve(&ctx.topic_or(None)?.slug())?;
        let title = arg_str_req(&input, "title")?;
        let raw_steps = input
            .get("steps")
            .and_then(|s| s.as_array())
            .ok_or_else(|| AppError::invalid("steps 不能为空"))?;

        let mut steps: Vec<LessonStep> = Vec::new();
        for raw in raw_steps {
            let step_title = arg_str(raw, "title").unwrap_or_default();
            if step_title.trim().is_empty() {
                continue;
            }
            steps.push(LessonStep {
                index: 0, // 保存时会按顺序重排
                title: step_title,
                focus: arg_str(raw, "focus").unwrap_or_default(),
                check: arg_str(raw, "check").unwrap_or_default(),
                sources: arg_str_array(raw, "sources"),
                status: StepStatus::Todo,
                demo_html: arg_str(raw, "demo_html"),
            });
        }
        if steps.is_empty() {
            return Err(AppError::invalid("每一步都需要一个 title"));
        }

        let plan: LessonPlan = save_plan_inner(
            &topic,
            LessonInput {
                id: arg_str(&input, "id"),
                title,
                goal: arg_str(&input, "goal").unwrap_or_default(),
                prereqs: arg_str_array(&input, "prereqs"),
                steps,
                finished: Some(false),
            },
        )?;

        core.emit_topics_updated(&topic);
        let mut out = format!("讲解方案已就绪（id {}）：\n\n{}", plan.id, plan.digest());
        out.push_str(
            "\n接下来按顺序讲：一次只推进**一步**，讲完那一步的 check 问题，等用户回答再走下一步。\n\
             每一步讲完记得调 lesson_step 标记成 done。",
        );
        Ok(ToolOutput::ok(out))
    }
}

pub struct LessonStepTool;

#[async_trait]
impl Tool for LessonStepTool {
    fn name(&self) -> &'static str {
        "lesson_step"
    }

    fn description(&self) -> &'static str {
        "更新讲解方案的进度：标记某一步「讲解中 / 已讲完 / 跳过」。\n\
         讲完一步就标 done 再进下一步；用户说「这步我懂了，跳过」时标 skipped。\n\
         进度会出现在系统提示词里，这一步不能忘。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "id": str_prop("方案 id"),
                "index": num_prop("第几步（从 1 开始）"),
                "status": {
                    "type": "string",
                    "enum": ["todo", "doing", "done", "skipped"],
                    "description": "默认 doing"
                },
            }),
            &["id", "index"],
        )
    }

    fn risk(&self) -> crate::agent::event::Risk {
        crate::agent::event::Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        format!(
            "更新讲解进度：第 {} 步 → {}",
            arg_u32(input, "index").unwrap_or(0),
            arg_str(input, "status").unwrap_or_else(|| "doing".into())
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let id = arg_str_req(&input, "id")?;
        let index = arg_u32(&input, "index").ok_or_else(|| AppError::invalid("缺少 index"))?;
        let status = match arg_str(&input, "status").as_deref() {
            Some("done") => StepStatus::Done,
            Some("skipped") => StepStatus::Skipped,
            Some("todo") => StepStatus::Todo,
            _ => StepStatus::Doing,
        };

        let path = topic.lessons_dir().join(format!("{id}.json"));
        let mut plan = store::read_json_opt::<LessonPlan>(&path)?
            .ok_or_else(|| AppError::NotFound(format!("找不到讲解方案 {id}")))?;
        if !plan.steps.iter().any(|s| s.index == index) {
            return Err(AppError::invalid(format!("方案里没有第 {index} 步")));
        }
        for s in plan.steps.iter_mut() {
            if s.index == index {
                s.status = status;
            } else if status == StepStatus::Doing && s.status == StepStatus::Doing {
                s.status = StepStatus::Todo;
            }
        }
        plan.updated_at = chrono::Utc::now();
        if plan
            .steps
            .iter()
            .all(|s| matches!(s.status, StepStatus::Done | StepStatus::Skipped))
        {
            plan.finished = true;
        }
        store::write_json(&path, &plan)?;

        let (done, total) = plan.progress();
        let next = plan
            .current_step()
            .map(|s| format!("下一步：第 {} 步 {}", s.index, s.title));
        Ok(ToolOutput::ok(format!(
            "已把第 {index} 步标记为{}。进度 {done}/{total}。{}",
            status.label(),
            next.unwrap_or_else(|| "方案已全部走完。".into())
        )))
    }
}
