//! 测验类工具：出题、看卷、判分。
//!
//! 出题的质量取决于「题型是否用对」：A1 考单一知识点、A2 考临床/场景推理、
//! B 型考一组易混概念、X 型考「哪些说法正确」。这里在工具描述里把这些约定写清楚，
//! 模型才不会把所有题都出成 A1。

use crate::agent::registry::{
    arg_str, arg_str_req, arg_u32, num_prop, object_schema, str_prop, Tool, ToolCtx, ToolOutput,
};
use crate::domain::quiz::{Answer, Attempt, Question, QuestionType, Quiz, QuizKind};
use crate::domain::topic::Topic;
use crate::error::{AppError, AppResult};
use crate::store;
use async_trait::async_trait;
use serde_json::{json, Value};

/// 试卷文件路径。id 由模型给出，同样要过 `is_safe_slug`（正常是 uuid，
/// 但拼文件名之前必须挡掉 `../` 这类越界——命令层一直是这么做的）。
fn quiz_file(topic: &Topic, quiz_id: &str) -> AppResult<std::path::PathBuf> {
    if !crate::paths::is_safe_slug(quiz_id) {
        return Err(AppError::invalid(format!("试卷 id 不合法：{quiz_id}")));
    }
    Ok(topic.quizzes_dir().join(format!("{quiz_id}.json")))
}

/// 读一份试卷的作答记录（用户在「工作台 → 测验」交卷时写进 `<id>.attempts.jsonl`）。
///
/// 为什么工具要读它：用户说「我做完了」时，agent 看不到作答与得分，只能反过来
/// 让用户手抄成绩单——而这些数据本来就在盘上，和界面判分用的是同一份记录。
fn load_attempts(topic: &Topic, quiz_id: &str) -> Vec<Attempt> {
    if !crate::paths::is_safe_slug(quiz_id) {
        return Vec::new();
    }
    let path = topic.quizzes_dir().join(format!("{quiz_id}.attempts.jsonl"));
    store::read_jsonl::<Attempt>(&path).unwrap_or_default()
}

/// 把一次作答逐题展开：用户自己的答案 + 对错 + 标准答案 / 采分点。
fn format_attempt(quiz: &Quiz, attempt: &Attempt) -> String {
    let when = attempt
        .finished_at
        .unwrap_or(attempt.started_at)
        .with_timezone(&chrono::Local)
        .format("%m-%d %H:%M");
    let mut out = format!(
        "最近一次作答（{when}）：{} / {} 分（{}%）\n",
        attempt.score,
        attempt.total,
        attempt.percent() as i64
    );
    for (i, q) in quiz.questions.iter().enumerate() {
        let given = attempt
            .answers
            .iter()
            .find(|a| a.question_id == q.id)
            .map(|a| a.value.clone())
            .unwrap_or_default();
        let result = attempt.results.iter().find(|r| r.question_id == q.id);
        let mark = match result.and_then(|r| r.correct) {
            Some(true) => "✓",
            Some(false) => "✗",
            None => "…",
        };
        out.push_str(&format!(
            "{}. {mark} 【{}】{} → {} / {} 分\n",
            i + 1,
            q.kind.label(),
            crate::agent::provider::truncate(&q.stem, 40),
            result.map(|r| r.score).unwrap_or(0.0),
            q.score
        ));
        if q.kind.is_subjective() {
            if given.trim().is_empty() {
                out.push_str("   未作答\n");
            } else {
                out.push_str(&format!("   你的作答：{}\n", crate::agent::provider::truncate(&given, 200)));
            }
            match result {
                Some(r) if r.graded_by_agent => {
                    out.push_str(&format!("   已评 {}/{} 分：{}\n", r.score, r.max_score, r.comment));
                    if !r.missing.is_empty() {
                        out.push_str(&format!("   漏掉的采分点：{}\n", r.missing.join("；")));
                    }
                }
                _ => out.push_str(&format!("   待评分，采分点：{}\n", q.key_points.join("；"))),
            }
        } else {
            if given.trim().is_empty() {
                out.push_str("   未作答\n");
            } else {
                out.push_str(&format!("   你的作答：{given}\n"));
            }
            if result.and_then(|r| r.correct) == Some(false) {
                out.push_str(&format!("   标准答案：{}\n", q.answer.join("")));
            }
        }
    }
    out
}

/// 选项 key 去重：模型给重复的（"A. xx" + "a. yy"、两条 {key:"A"}）时，
/// 按序号字母兜底，再撞就循环加后缀直到空位——key 不唯一的话判分按 key
/// 找会定位错行。i ≥ 26 时别玩 `b'A' + i` 的花活（debug 溢出 panic）。
fn unique_key(used: &mut std::collections::HashSet<String>, mut key: String, i: usize) -> String {
    if used.contains(&key) {
        key = if i < 26 {
            ((b'A' + i as u8) as char).to_string()
        } else {
            format!("O{}", i + 1)
        };
        let mut n = i + 1;
        while used.contains(&key) {
            key = format!("{key}{n}");
            n += 1;
        }
    }
    used.insert(key.clone());
    key
}

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
        let mut used_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (i, o) in opts.iter().enumerate() {
            // 选项允许写成 "A. 文本" / "(A) 文本" 的字符串，或 {key, text} 对象
            match o {
                Value::String(s) => {
                    // 只认「单个字母（可带括号）+ 分隔符」的前缀。之前按
                    // 「字母/点/顿号/空格」整串剥，选项以拉丁字母开头时
                    // （ARDS、TCP 连接）正文被剥光、key 也取错——判分全错。
                    let trimmed = s.trim();
                    let mut key = ((b'A' + i as u8) as char).to_string();
                    let mut text = trimmed.to_string();
                    let chars: Vec<char> = trimmed.chars().collect();
                    // (A) 文本 / （A）文本
                    let paren = chars.len() >= 4
                        && (chars[0] == '(' || chars[0] == '（')
                        && chars[1].is_ascii_alphabetic()
                        && (chars[2] == ')' || chars[2] == '）');
                    if paren {
                        key = chars[1].to_ascii_uppercase().to_string();
                        text = trimmed[chars[0].len_utf8() + chars[1].len_utf8() + chars[2].len_utf8()..]
                            .trim()
                            .to_string();
                    } else if chars.len() >= 2 && chars[0].is_ascii_alphabetic() {
                        let sep = chars[1] == '.'
                            || chars[1] == '、'
                            || chars[1] == ')'
                            || chars[1] == '）'
                            || (chars[1] == ' ' && chars.len() > 2);
                        if sep {
                            key = chars[0].to_ascii_uppercase().to_string();
                            text = trimmed[chars[0].len_utf8() + chars[1].len_utf8()..].trim().to_string();
                        }
                    }
                    let key = unique_key(&mut used_keys, key, i);
                    q.options.push(crate::domain::quiz::QuizOption { key, text });
                }
                Value::Object(_) => {
                    // 对象形式的 key 同样要参与去重（模型给两条 {key:"A"} 是常事）
                    let key = arg_str(o, "key")
                        .or_else(|| arg_str(o, "label"))
                        .unwrap_or_else(|| ((b'A' + i as u8) as char).to_string());
                    let key = unique_key(&mut used_keys, key, i);
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
                let attempts = load_attempts(&topic, &quiz.id);
                let tail = match attempts.last() {
                    Some(a) => format!("｜作答 {} 次，最近 {}%", attempts.len(), a.percent() as i64),
                    None => "｜未作答".to_string(),
                };
                lines.push(format!(
                    "- {}｜{}｜{} 题 / {} 分{}｜id={}",
                    quiz.title,
                    quiz.type_summary(),
                    quiz.questions.len(),
                    quiz.total_score(),
                    tail,
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
        "读一份试卷的完整内容、标准答案，以及**用户最近的作答记录**（交卷时间、得分、\
         逐题对错、主观题待评与采分点）。用户说「做完了 / 考完了」时先用它拿真实成绩，\
         不要反过来让用户手抄分数。"
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
        let quiz = store::read_json_opt::<Quiz>(&quiz_file(&topic, &id)?)?
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

        // 作答记录：用户「在测验页做完了」之后，agent 靠这一段拿到真实成绩与错题，
        // 而不是让用户手抄（这条曾漏掉，模型只能反过来问用户要分数）
        let attempts = load_attempts(&topic, &id);
        if attempts.is_empty() {
            out.push_str("\n（还没有作答记录——用户在「工作台 → 测验」里交卷后，这里会出现成绩与逐题对错。）\n");
        } else {
            out.push_str(&format!("\n—— 作答记录（共 {} 次）——\n", attempts.len()));
            if attempts.len() > 1 {
                let earlier: Vec<String> = attempts[..attempts.len() - 1]
                    .iter()
                    .map(|a| format!("{}%", a.percent() as i64))
                    .collect();
                out.push_str(&format!("此前的得分：{}\n\n", earlier.join("、")));
            }
            if let Some(last) = attempts.last() {
                out.push_str(&format_attempt(&quiz, last));
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
        let quiz = store::read_json_opt::<Quiz>(&quiz_file(&topic, &id)?)?
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
        // 与界面判卷同一份记录：不落盘的话这次判分在历史/统计里查无此事，
        // quiz_list 承诺的「最近得分」对这条路径永远是空的
        let _op = store::file_op_lock();
        store::append_jsonl(
            &topic.quizzes_dir().join(format!("{}.attempts.jsonl", quiz.id)),
            &attempt,
        )?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::quiz::QuizOption;

    fn sample_quiz() -> (Quiz, Question, Question, Question) {
        let mut quiz = Quiz::new("t1", "随堂小测");
        let mut a1 = Question::new(QuestionType::A1, "哪个说法正确？");
        a1.options = vec![
            QuizOption { key: "A".into(), text: "甲".into() },
            QuizOption { key: "B".into(), text: "乙".into() },
        ];
        a1.answer = vec!["B".into()];
        let mut x = Question::new(QuestionType::X, "哪些说法对？");
        x.options = vec![
            QuizOption { key: "A".into(), text: "甲".into() },
            QuizOption { key: "B".into(), text: "乙".into() },
            QuizOption { key: "C".into(), text: "丙".into() },
        ];
        x.answer = vec!["A".into(), "C".into()];
        let mut short = Question::new(QuestionType::Short, "解释一下");
        short.score = 5;
        short.key_points = vec!["要点一".into(), "要点二".into()];
        quiz.questions = vec![a1.clone(), x.clone(), short.clone()];
        (quiz, a1, x, short)
    }

    /// 用户「在测验页做完了」时 agent 要能看到的东西：对错、自己写的答案、
    /// 标准答案、主观题的待评采分点。曾经这些全缺，模型只能反过来问用户要分数。
    #[test]
    fn attempt_digest_shows_answers_and_pending_points() {
        let (quiz, a1, x, short) = sample_quiz();
        let mut attempt = Attempt::new(&quiz.id);
        attempt.answers = vec![
            Answer { question_id: a1.id.clone(), value: "B".into() },
            Answer { question_id: x.id.clone(), value: "AB".into() },
            Answer { question_id: short.id.clone(), value: "我的作答原文".into() },
        ];
        attempt.grade_objective(&quiz);

        let text = format_attempt(&quiz, &attempt);
        assert!(text.contains("✓"), "答对要有勾：{text}");
        assert!(text.contains("✗"), "答错要有叉：{text}");
        assert!(text.contains("你的作答：AB"), "要带上用户自己的答案：{text}");
        assert!(text.contains("标准答案：AC"), "错题要给出标准答案：{text}");
        assert!(text.contains("我的作答原文"), "主观题要带上用户原文：{text}");
        assert!(text.contains("待评分，采分点：要点一；要点二"), "主观题要给采分点：{text}");
    }

    #[test]
    fn graded_subjective_shows_comment_and_missing_points() {
        let (quiz, a1, x, short) = sample_quiz();
        let mut attempt = Attempt::new(&quiz.id);
        attempt.answers = vec![
            Answer { question_id: a1.id.clone(), value: "B".into() },
            Answer { question_id: x.id.clone(), value: "AC".into() },
            Answer { question_id: short.id.clone(), value: "只答对一半".into() },
        ];
        attempt.grade_objective(&quiz);
        attempt.apply_subjective(&short.id, 3.0, "答对一半".into(), vec!["要点二".into()]);

        let text = format_attempt(&quiz, &attempt);
        assert!(text.contains("已评 3/5 分：答对一半"), "{text}");
        assert!(text.contains("漏掉的采分点：要点二"), "{text}");
    }

    /// id 来自模型，拼文件名前必须过安全校验（与命令层同一条防线）。
    #[test]
    fn quiz_id_guard_rejects_traversal() {
        assert!(!crate::paths::is_safe_slug("../../x"));
        assert!(!crate::paths::is_safe_slug("a/b"));
        assert!(crate::paths::is_safe_slug("b2f5f712-236b-41b3-ada3-77633cc12c8d"));
    }
}
