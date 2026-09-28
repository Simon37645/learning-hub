//! 测验类工具：出题、看卷、判分。
//!
//! 出题的质量取决于「题型是否用对」：A1 考单一知识点、A2 考临床/场景推理、
//! B 型考一组易混概念、X 型考「哪些说法正确」。这里在工具描述里把这些约定写清楚，
//! 模型才不会把所有题都出成 A1。

use crate::agent::registry::{
    arg_str, arg_str_req, arg_u32, num_prop, object_schema, str_prop, Tool, ToolCtx, ToolOutput,
};
use crate::domain::quiz::{Answer, Question, QuestionType, Quiz, QuizKind};
use crate::error::{AppError, AppResult};
use crate::store;
use async_trait::async_trait;
use serde_json::{json, Value};

fn parse_question(raw: &Value) -> AppResult<Question> {
    let kind_raw = arg_str(raw, "type")
        .or_else(|| arg_str(raw, "kind"))
        .unwrap_or_else(|| "A1".into());
    let kind = QuestionType::parse(&kind_raw)
        .ok_or_else(|| AppError::invalid(format!("未知题型：{kind_raw}")))?;
    let stem = arg_str_req(raw, "stem")?;
    let mut q = Question::new(kind, stem);
    q.case_text = arg_str(raw, "case_text").or_else(|| arg_str(raw, "caseText"));
    q.explanation = arg_str(raw, "explanation").unwrap_or_default();
    q.source = arg_str(raw, "source");
    q.group = arg_str(raw, "group");
    q.score = arg_u32(raw, "score").unwrap_or(if kind.is_subjective() { 5 } else { 1 });

    if let Some(opts) = raw.get("options").and_then(|o| o.as_array()) {
        for (i, o) in opts.iter().enumerate() {
            // 选项允许写成 "A. 文本" 的字符串，或 {key, text} 对象
            match o {
                Value::String(s) => {
                    let key = s
                        .chars()
                        .next()
                        .filter(|c| c.is_ascii_alphabetic())
                        .map(|c| c.to_ascii_uppercase().to_string())
                        .unwrap_or_else(|| ((b'A' + i as u8) as char).to_string());
                    let text = s
                        .trim_start_matches(|c: char| c.is_ascii_alphabetic() || c == '.' || c == '、' || c == ' ')
                        .to_string();
                    q.options.push(crate::domain::quiz::QuizOption { key, text });
                }
                Value::Object(_) => {
                    let key = arg_str(o, "key")
                        .or_else(|| arg_str(o, "label"))
                        .unwrap_or_else(|| ((b'A' + i as u8) as char).to_string());
                    q.options
                        .push(crate::domain::quiz::QuizOption { key, text: arg_str(o, "text").unwrap_or_default() });
                }
                _ => {}
            }
        }
    }

    q.answer = crate::agent::registry::arg_str_array(raw, "answer");
    if q.answer.is_empty() {
        if let Some(a) = arg_str(raw, "answer") {
            q.answer = vec![a];
        }
    }
    q.key_points = crate::agent::registry::arg_str_array(raw, "key_points");
    Ok(q)
}

pub struct QuizCreate;

#[async_trait]
impl Tool for QuizCreate {
    fn name(&self) -> &'static str {
        "quiz_create"
    }

    fn description(&self) -> &'static str {
        "把出好的题目存成一份测验试卷（用户可以在「工作台 → 测验」里作答并自动判分）。\n\
         题型用 type 指定，请按考察目标选对题型：\n\
         - A1：单知识点最佳选择题。options 给 5 个，answer 给一个选项号\n\
         - A2：场景/病例推理题。case_text 写场景，stem 写问题，options 5 个，answer 一个\n\
         - B：标准配伍题（一组选项配若干小题）。同一组的小题传相同 group，options 只写在第一题上，每题给一个 answer\n\
         - X：多项选择题。answer 给所有正确选项号（如 [\"A\",\"C\",\"D\"]）\n\
         - Term：名词解释。不给 options，用 key_points 列采分点\n\
         - Short：简答题。不给 options，用 key_points 列采分点\n\
         客观题必须给标准答案，主观题必须给采分点（判分靠它逐条对照）。\n\
         一次出 5~10 题即可，不要为了凑数出重复题。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "title": str_prop("试卷标题，例如「特征值专项小测」"),
                "scope": str_prop("覆盖范围说明，例如「特征值与特征向量，第 1~3 节」"),
                "kind": { "type": "string", "enum": ["practice", "exam"], "description": "随堂练习 / 模拟考试，默认 practice" },
                "questions": {
                    "type": "array",
                    "description": "题目列表",
                    "items": {
                        "type": "object",
                        "properties": {
                            "type": { "type": "string", "enum": ["A1", "A2", "B", "X", "Term", "Short"] },
                            "stem": str_prop("题干"),
                            "case_text": str_prop("A2 型的场景/病例摘要"),
                            "options": { "type": "array", "items": { "type": "string" }, "description": "选项，可写 [\"A. …\",\"B. …\"] 或 [\"…\",\"…\"]" },
                            "answer": { "type": "array", "items": { "type": "string" }, "description": "客观题的标准答案选项号" },
                            "key_points": { "type": "array", "items": { "type": "string" }, "description": "主观题采分点" },
                            "explanation": str_prop("解析"),
                            "source": str_prop("出处"),
                            "score": num_prop("分值，客观题默认 1，主观题默认 5"),
                            "group": str_prop("B 型题的共用选项组名"),
                        },
                    },
                },
            }),
            &["title", "questions"],
        )
    }

    fn risk(&self) -> crate::agent::event::Risk {
        crate::agent::event::Risk::Write
    }

    fn summarize(&self, input: &Value) -> String {
        let n = input
            .get("questions")
            .and_then(|q| q.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        format!(
            "出卷「{}」（{n} 题）",
            arg_str(input, "title").unwrap_or_default()
        )
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let core = ctx.core.clone();
        let topic = core.workspace().resolve(&ctx.topic_or(None)?.slug())?;
        let title = arg_str_req(&input, "title")?;
        let raw_questions = input
            .get("questions")
            .and_then(|q| q.as_array())
            .ok_or_else(|| AppError::invalid("questions 不能为空"))?;

        let mut quiz = Quiz::new(topic.meta.id.clone(), title.clone());
        quiz.scope = arg_str(&input, "scope").unwrap_or_default();
        quiz.kind = match arg_str(&input, "kind").as_deref() {
            Some("exam") => QuizKind::Exam,
            _ => QuizKind::Practice,
        };
        let mut problems: Vec<String> = Vec::new();
        for raw in raw_questions {
            match parse_question(raw) {
                Ok(q) => {
                    // 客观题必须有答案，否则判分时永远是错的
                    if !q.kind.is_subjective() && q.answer.is_empty() {
                        problems.push(format!("「{}」缺标准答案，已跳过", crate::agent::provider::truncate(&q.stem, 24)));
                        continue;
                    }
                    if q.kind.is_subjective() && q.key_points.is_empty() {
                        problems.push(format!("「{}」缺采分点，已跳过", crate::agent::provider::truncate(&q.stem, 24)));
                        continue;
                    }
                    quiz.questions.push(q);
                }
                Err(e) => problems.push(format!("有一题解析失败：{e}")),
            }
        }
        if quiz.questions.is_empty() {
            return Err(AppError::invalid(format!(
                "没有一道题是可用的。{}",
                problems.join("；")
            )));
        }

        let dir = topic.quizzes_dir();
        crate::paths::ensure_dir(&dir)?;
        store::write_json(&dir.join(format!("{}.json", quiz.id)), &quiz)?;
        core.emit_topics_updated(&topic);

        let mut out = format!(
            "已生成试卷「{}」：{}，共 {} 题 / {} 分。\n用户在「工作台 → 测验」里就能作答，客观题交卷即出分。",
            quiz.title,
            quiz.type_summary(),
            quiz.questions.len(),
            quiz.total_score()
        );
        if !problems.is_empty() {
            out.push_str(&format!("\n\n有 {} 题被跳过：{}", problems.len(), problems.join("；")));
        }
        out.push_str("\n\n出完卷后请邀请用户去测验页作答，不要在这里直接把答案念出来。");
        Ok(ToolOutput::ok(out))
    }
}

pub struct QuizList;

#[async_trait]
impl Tool for QuizList {
    fn name(&self) -> &'static str {
        "quiz_list"
    }

    fn description(&self) -> &'static str {
        "列出这个主题下已有的试卷与最近得分。想复盘「上次错在哪」时先看它。"
    }

    fn schema(&self) -> Value {
        object_schema(json!({}), &[])
    }

    fn summarize(&self, _input: &Value) -> String {
        "查看已有测验".into()
    }

    async fn run(&self, ctx: &ToolCtx, _input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let dir = topic.quizzes_dir();
        if !dir.is_dir() {
            return Ok(ToolOutput::ok("这个主题还没有测验。可以用 quiz_create 出一份。"));
        }
        let mut lines = Vec::new();
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|e| e != "json") {
                continue;
            }
            if let Ok(Some(quiz)) = store::read_json_opt::<Quiz>(&path) {
                lines.push(format!(
                    "- {}｜{}｜{} 题 / {} 分｜id={}",
                    quiz.title,
                    quiz.type_summary(),
                    quiz.questions.len(),
                    quiz.total_score(),
                    quiz.id
                ));
            }
        }
        if lines.is_empty() {
            return Ok(ToolOutput::ok("还没有测验。"));
        }
        Ok(ToolOutput::ok(format!("共 {} 份试卷：\n{}", lines.len(), lines.join("\n"))))
    }
}

pub struct QuizGet;

#[async_trait]
impl Tool for QuizGet {
    fn name(&self) -> &'static str {
        "quiz_get"
    }

    fn description(&self) -> &'static str {
        "读一份试卷的完整内容与标准答案。用户做完题把它拿进来，就能逐题讲解。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({ "quiz_id": str_prop("试卷 id，来自 quiz_list") }),
            &["quiz_id"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        format!("查看试卷 {}", arg_str(input, "quiz_id").unwrap_or_default())
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let id = arg_str_req(&input, "quiz_id")?;
        let path = topic.quizzes_dir().join(format!("{id}.json"));
        let quiz = store::read_json_opt::<Quiz>(&path)?
            .ok_or_else(|| AppError::NotFound(format!("找不到试卷 {id}")))?;

        let mut out = format!(
            "试卷：{}（{}）\n覆盖：{}\n\n",
            quiz.title,
            quiz.type_summary(),
            if quiz.scope.is_empty() { "未标注" } else { &quiz.scope }
        );
        for (i, q) in quiz.questions.iter().enumerate() {
            out.push_str(&format!("{}. 【{}】{}（{} 分）\n", i + 1, q.kind.label(), q.stem, q.score));
            if let Some(c) = &q.case_text {
                out.push_str(&format!("   场景：{c}\n"));
            }
            for o in &q.options {
                out.push_str(&format!("   {}. {}\n", o.key, o.text));
            }
            if !q.answer.is_empty() {
                out.push_str(&format!("   答案：{}\n", q.answer.join("")));
            }
            if !q.key_points.is_empty() {
                out.push_str(&format!("   采分点：{}\n", q.key_points.join("；")));
            }
            if !q.explanation.is_empty() {
                out.push_str(&format!("   解析：{}\n", q.explanation));
            }
        }
        Ok(ToolOutput::ok(out))
    }
}

pub struct QuizGrade;

#[async_trait]
impl Tool for QuizGrade {
    fn name(&self) -> &'static str {
        "quiz_grade"
    }

    fn description(&self) -> &'static str {
        "用户把答案发在对话里时，用它按标准答案与采分点判分。\n\
         answers 里每项是 {question_id, value}：客观题 value 写选项号（如 \"AC\"），主观题写用户的原文。\n\
         先用 quiz_get 拿到题目 id，再调用它。"
    }

    fn schema(&self) -> Value {
        object_schema(
            json!({
                "quiz_id": str_prop("试卷 id"),
                "answers": {
                    "type": "array",
                    "items": { "type": "object" },
                    "description": "[{question_id, value}]"
                },
            }),
            &["quiz_id", "answers"],
        )
    }

    fn summarize(&self, input: &Value) -> String {
        let n = input
            .get("answers")
            .and_then(|a| a.as_array())
            .map(|a| a.len())
            .unwrap_or(0);
        format!("批改 {n} 道题")
    }

    async fn run(&self, ctx: &ToolCtx, input: Value) -> AppResult<ToolOutput> {
        let topic = ctx.topic()?;
        let id = arg_str_req(&input, "quiz_id")?;
        let path = topic.quizzes_dir().join(format!("{id}.json"));
        let quiz = store::read_json_opt::<Quiz>(&path)?
            .ok_or_else(|| AppError::NotFound(format!("找不到试卷 {id}")))?;

        let raw = input
            .get("answers")
            .and_then(|a| a.as_array())
            .cloned()
            .unwrap_or_default();
        let answers: Vec<Answer> = raw
            .iter()
            .filter_map(|a| {
                let qid = arg_str(a, "question_id")?;
                let value = arg_str(a, "value").unwrap_or_default();
                Some(Answer { question_id: qid, value })
            })
            .collect();

        // 只做客观题本地判分：主观题在界面上由模型按采分点评，结果落盘才有意义
        let mut attempt = crate::domain::quiz::Attempt::new(&quiz.id);
        attempt.answers = answers;
        attempt.grade_objective(&quiz);

        let mut out = format!(
            "判分结果（{}）：{} / {} 分\n\n",
            quiz.title,
            attempt.score,
            attempt.total
        );
        for (i, q) in quiz.questions.iter().enumerate() {
            let r = attempt.results.iter().find(|r| r.question_id == q.id);
            let Some(r) = r else { continue };
            let mark = match r.correct {
                Some(true) => "✓",
                Some(false) => "✗",
                None => "…",
            };
            out.push_str(&format!(
                "{}. {mark} 【{}】{} → {} 分\n",
                i + 1,
                q.kind.label(),
                crate::agent::provider::truncate(&q.stem, 40),
                r.score
            ));
            if !q.answer.is_empty() && r.correct == Some(false) {
                out.push_str(&format!("   标准答案：{}\n", q.answer.join("")));
            }
        }
        if !attempt.pending_subjective.is_empty() {
            out.push_str(&format!(
                "\n还有 {} 道主观题需要按采分点评：请在回复里逐条对照采分点给分，并指出漏掉的点。",
                attempt.pending_subjective.len()
            ));
        }
        Ok(ToolOutput::ok(out))
    }
}
