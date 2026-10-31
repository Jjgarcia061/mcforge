//! Conversations with the engine chosen in the setup, which can use MCForge's
//! own tools (the library, databases, reports) under the user's permission rules.

pub mod store;
pub mod tools;

use std::collections::HashSet;

use futures_util::StreamExt;
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::tools::Tools;
use crate::error::Result;
use crate::policy::Permission;
use crate::providers::stream::{ChatModel, StreamEvent};

/// Model calls per turn; a model that keeps calling tools is stopped here.
pub const MAX_STEPS: usize = 8;
/// Tool output older than the latest question is shortened to this.
const OLD_OUTPUT_CHARS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    User,
    Assistant,
}

/// One turn of a conversation. Tool calls are assistant parts; their results
/// travel back in the next user message, as the providers expect.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct Message {
    pub role: Role,
    pub parts: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Part {
    Text {
        text: String,
    },
    ToolCall {
        id: String,
        name: String,
        #[specta(type = specta_typescript::Unknown)]
        input: Value,
    },
    ToolResult {
        id: String,
        /// What the model reads.
        output: String,
        #[serde(rename = "isError")]
        is_error: bool,
        /// What the user sees; never sent to the model.
        #[serde(default)]
        view: Option<ToolView>,
    },
}

/// How a tool call went, for the conversation on screen. The UI words it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct ToolView {
    pub decision: Decision,
    pub summary: Summary,
    /// Rows the tool returned, as text.
    pub table: Option<Table>,
    /// Library documents the tool drew on.
    #[serde(default)]
    pub sources: Vec<SourceRef>,
    /// A report the tool generated.
    pub report: Option<ReportRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Decision {
    /// Allowed by the user's rules.
    Allowed,
    /// The rules ask first, and the user said yes.
    Approved,
    /// The rules ask first, and the user said no.
    Rejected,
    /// Blocked by the user's rules.
    Blocked,
}

/// What a tool did, as data.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Summary {
    Passages {
        count: u32,
        documents: u32,
    },
    Rows {
        count: u32,
        truncated: bool,
    },
    Tables {
        count: u32,
    },
    Columns {
        count: u32,
    },
    Changed {
        rows: u32,
    },
    Templates {
        count: u32,
    },
    Report {
        sections: u32,
    },
    Error {
        message: String,
    },
    /// Not run: blocked or declined.
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct SourceRef {
    pub title: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct ReportRef {
    pub path: String,
    pub title: String,
}

impl Message {
    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            parts: vec![Part::Text { text: text.into() }],
        }
    }
}

/// Streamed to the UI while a turn runs.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ChatEvent {
    Text {
        text: String,
    },
    ToolStarted {
        id: String,
        name: String,
        #[specta(type = specta_typescript::Unknown)]
        input: Value,
    },
    /// The rules ask first: the turn waits for `answer_approval`.
    ApprovalNeeded {
        id: String,
        name: String,
        #[specta(type = specta_typescript::Unknown)]
        input: Value,
    },
    ToolFinished {
        id: String,
        #[serde(rename = "isError")]
        is_error: bool,
        view: ToolView,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Approval {
    Once,
    /// Allow this tool from now on: the host saves the rule.
    Always,
    Reject,
}

/// One tool call, for the activity log.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct Activity {
    /// RFC 3339.
    pub at: String,
    pub tool: String,
    /// The input, shortened.
    pub input: String,
    pub decision: Decision,
    pub ok: bool,
}

/// What a turn needs from the app around it.
pub trait Host: Send + Sync {
    fn emit(&self, event: ChatEvent);
    /// Waits for the user's answer about a call the rules ask about.
    fn approve<'a>(&'a self, id: &'a str, name: &'a str) -> BoxFuture<'a, Approval>;
    fn record(&self, activity: Activity);
}

/// Runs one turn: the model answers the conversation, calling tools as needed,
/// until it stops or `MAX_STEPS` is reached. Everything it says and does is
/// appended to `history` as it completes, so a cancelled turn keeps its steps.
pub async fn run_turn(
    model: &dyn ChatModel,
    tools: &dyn Tools,
    host: &dyn Host,
    system: &str,
    budget: usize,
    history: &mut Vec<Message>,
) -> Result<()> {
    let specs = tools.specs();
    // Tools the user allowed for good during this turn.
    let mut always: HashSet<String> = HashSet::new();
    for _ in 0..MAX_STEPS {
        let context = window(history, budget);
        let mut stream = model.stream(system, &context, &specs);
        let mut text = String::new();
        let mut calls = Vec::new();
        while let Some(event) = stream.next().await {
            match event? {
                StreamEvent::Text(t) => {
                    host.emit(ChatEvent::Text { text: t.clone() });
                    text.push_str(&t);
                }
                StreamEvent::ToolCall { id, name, input } => calls.push((id, name, input)),
            }
        }
        drop(stream);
        let mut parts = Vec::new();
        if !text.trim().is_empty() {
            parts.push(Part::Text { text });
        }
        parts.extend(calls.iter().map(|(id, name, input)| Part::ToolCall {
            id: id.clone(),
            name: name.clone(),
            input: input.clone(),
        }));
        history.push(Message {
            role: Role::Assistant,
            parts,
        });
        if calls.is_empty() {
            return Ok(());
        }

        let mut results = Vec::new();
        for (id, name, input) in calls {
            host.emit(ChatEvent::ToolStarted {
                id: id.clone(),
                name: name.clone(),
                input: input.clone(),
            });
            let decision = match tools.permission(&name) {
                Permission::Allow => Decision::Allowed,
                Permission::Deny => Decision::Blocked,
                Permission::Ask if always.contains(&name) => Decision::Approved,
                Permission::Ask => {
                    host.emit(ChatEvent::ApprovalNeeded {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    });
                    match host.approve(&id, &name).await {
                        Approval::Once => Decision::Approved,
                        Approval::Always => {
                            always.insert(name.clone());
                            Decision::Approved
                        }
                        Approval::Reject => Decision::Rejected,
                    }
                }
            };
            let (output, is_error, view) = match decision {
                Decision::Allowed | Decision::Approved => match tools.run(&name, &input).await {
                    Ok(out) => (
                        out.text,
                        false,
                        ToolView {
                            decision,
                            summary: out.summary,
                            table: out.table,
                            sources: out.sources,
                            report: out.report,
                        },
                    ),
                    Err(e) => (format!("Error: {e}"), true, skipped(decision, Summary::Error { message: e.to_string() })),
                },
                Decision::Blocked => (
                    "Blocked by the user's rules. Do not try again or work around it; tell the user.".into(),
                    true,
                    skipped(decision, Summary::Skipped),
                ),
                Decision::Rejected => (
                    "The user declined this action. Do not try again unless they ask.".into(),
                    true,
                    skipped(decision, Summary::Skipped),
                ),
            };
            host.record(Activity {
                at: chrono::Local::now().to_rfc3339(),
                tool: name,
                input: input.to_string().chars().take(300).collect(),
                decision,
                ok: !is_error,
            });
            host.emit(ChatEvent::ToolFinished {
                id: id.clone(),
                is_error,
                view: view.clone(),
            });
            results.push(Part::ToolResult {
                id,
                output,
                is_error,
                view: Some(view),
            });
        }
        history.push(Message {
            role: Role::User,
            parts: results,
        });
    }
    Ok(())
}

/// The instructions every turn starts with.
pub fn system_prompt(language: &str, today: &str, attachments: &[tools::Attachment]) -> String {
    let mut prompt = format!(
        "You are the assistant inside MCForge, working for the user with their own documents and data. \
         Today is {today}. Answer in {language} unless the user writes in another language, and speak to the user \
         directly (\"your documents\", not \"the user's documents\").\n\
         - Use your tools instead of guessing: search the user's library when their documents may hold the answer, \
         and query their database for figures. Never invent figures, names or sources.\n\
         - When a fact comes from the library, name the document it comes from.\n\
         - Only change the database when the user explicitly asks; each change needs their approval.\n\
         - If a tool is blocked or the user declines it, say so plainly and do not work around it.\n\
         - Be brief and concrete; use a list or a small table when it helps."
    );
    if !attachments.is_empty() {
        let files: Vec<String> = attachments
            .iter()
            .map(|a| match &a.sheet {
                Some(sheet) => format!("{} (sheet {sheet})", a.name),
                None => a.name.clone(),
            })
            .collect();
        prompt.push_str(&format!(
            "\nFiles the user attached, which the report tool can use: {}.",
            files.join(", ")
        ));
    }
    prompt
}

fn skipped(decision: Decision, summary: Summary) -> ToolView {
    ToolView {
        decision,
        summary,
        table: None,
        sources: Vec::new(),
        report: None,
    }
}

fn is_question(m: &Message) -> bool {
    m.role == Role::User && m.parts.iter().any(|p| matches!(p, Part::Text { .. }))
}

fn size(m: &Message) -> usize {
    m.parts
        .iter()
        .map(|p| match p {
            Part::Text { text } => text.len(),
            Part::ToolCall { input, .. } => input.to_string().len() + 40,
            Part::ToolResult { output, .. } => output.len(),
        })
        .sum()
}

/// The latest messages that fit in `budget` characters, starting at a question
/// of the user's (never at a tool result), with tool output from before the
/// latest question shortened.
fn window(history: &[Message], budget: usize) -> Vec<Message> {
    let mut start = history.len();
    let mut used = 0;
    while start > 0 && used + size(&history[start - 1]) <= budget {
        start -= 1;
        used += size(&history[start]);
    }
    // The latest question always goes, even when it alone is over budget, and
    // the window starts at a question rather than in the middle of a turn.
    let latest = history.iter().rposition(is_question).unwrap_or(0);
    start = start.min(latest);
    while start < latest && !is_question(&history[start]) {
        start += 1;
    }
    history[start..]
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let mut m = m.clone();
            if start + i < latest {
                for p in &mut m.parts {
                    if let Part::ToolResult { output, .. } = p
                        && output.chars().count() > OLD_OUTPUT_CHARS
                    {
                        *output = format!(
                            "{}…",
                            output.chars().take(OLD_OUTPUT_CHARS).collect::<String>()
                        );
                    }
                }
            }
            m
        })
        .collect()
}

/// After a cancelled turn, answers any tool call left without a result, so
/// the conversation stays valid for the providers.
pub fn repair(history: &mut Vec<Message>) {
    let Some(last) = history.last() else { return };
    if last.role != Role::Assistant {
        return;
    }
    let results: Vec<Part> = last
        .parts
        .iter()
        .filter_map(|p| match p {
            Part::ToolCall { id, .. } => Some(Part::ToolResult {
                id: id.clone(),
                output: "Cancelled by the user.".into(),
                is_error: true,
                view: Some(skipped(Decision::Rejected, Summary::Skipped)),
            }),
            _ => None,
        })
        .collect();
    if !results.is_empty() {
        history.push(Message {
            role: Role::User,
            parts: results,
        });
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, VecDeque};
    use std::sync::Mutex;

    use futures_util::stream::{self, BoxStream};
    use serde_json::json;

    use super::tools::ToolOutput;
    use super::*;
    use crate::providers::stream::ToolSpec;

    /// A model that plays one scripted answer per call and remembers what it was sent.
    struct Script {
        answers: Mutex<VecDeque<Vec<StreamEvent>>>,
        seen: Mutex<Vec<usize>>,
    }

    impl Script {
        fn new(answers: Vec<Vec<StreamEvent>>) -> Self {
            Self {
                answers: Mutex::new(answers.into()),
                seen: Mutex::default(),
            }
        }
    }

    impl ChatModel for Script {
        fn stream<'a>(
            &'a self,
            _: &'a str,
            messages: &'a [Message],
            _: &'a [ToolSpec],
        ) -> BoxStream<'a, Result<StreamEvent>> {
            self.seen.lock().unwrap().push(messages.len());
            let events = self.answers.lock().unwrap().pop_front().unwrap_or_default();
            Box::pin(stream::iter(events.into_iter().map(Ok)))
        }
    }

    struct Fake {
        rules: HashMap<&'static str, Permission>,
        runs: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(rules: &[(&'static str, Permission)]) -> Self {
            Self {
                rules: rules.iter().copied().collect(),
                runs: Mutex::default(),
            }
        }
    }

    impl Tools for Fake {
        fn specs(&self) -> Vec<ToolSpec> {
            Vec::new()
        }

        fn permission(&self, name: &str) -> Permission {
            self.rules.get(name).copied().unwrap_or(Permission::Deny)
        }

        fn run<'a>(&'a self, name: &'a str, _: &'a Value) -> BoxFuture<'a, Result<ToolOutput>> {
            self.runs.lock().unwrap().push(name.to_owned());
            Box::pin(async move {
                Ok(ToolOutput {
                    text: format!("{name} done"),
                    summary: Summary::Tables { count: 1 },
                    table: None,
                    sources: Vec::new(),
                    report: None,
                })
            })
        }
    }

    #[derive(Default)]
    struct Recorder {
        events: Mutex<Vec<ChatEvent>>,
        answers: Mutex<VecDeque<Approval>>,
        log: Mutex<Vec<Activity>>,
    }

    impl Host for Recorder {
        fn emit(&self, event: ChatEvent) {
            self.events.lock().unwrap().push(event);
        }

        fn approve<'a>(&'a self, _: &'a str, _: &'a str) -> BoxFuture<'a, Approval> {
            let answer = self
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Approval::Reject);
            Box::pin(async move { answer })
        }

        fn record(&self, activity: Activity) {
            self.log.lock().unwrap().push(activity);
        }
    }

    fn call(id: &str, name: &str) -> StreamEvent {
        StreamEvent::ToolCall {
            id: id.into(),
            name: name.into(),
            input: json!({ "q": id }),
        }
    }

    fn text(t: &str) -> StreamEvent {
        StreamEvent::Text(t.into())
    }

    fn kinds(host: &Recorder) -> Vec<&'static str> {
        host.events
            .lock()
            .unwrap()
            .iter()
            .map(|e| match e {
                ChatEvent::Text { .. } => "text",
                ChatEvent::ToolStarted { .. } => "started",
                ChatEvent::ApprovalNeeded { .. } => "approval",
                ChatEvent::ToolFinished { .. } => "finished",
            })
            .collect()
    }

    #[tokio::test]
    async fn a_tool_result_goes_back_to_the_model() {
        let model = Script::new(vec![
            vec![text("Busco"), call("t1", "library_search")],
            vec![text("La meta es 1.2 millones (metas-2026).")],
        ]);
        let tools = Fake::new(&[("library_search", Permission::Allow)]);
        let host = Recorder::default();
        let mut history = vec![Message::text(Role::User, "¿Meta del Norte?")];
        run_turn(&model, &tools, &host, "sys", 10_000, &mut history)
            .await
            .unwrap();

        assert_eq!(history.len(), 4);
        assert!(
            matches!(&history[2].parts[0], Part::ToolResult { output, is_error: false, .. } if output == "library_search done")
        );
        assert_eq!(kinds(&host), ["text", "started", "finished", "text"]);
        assert_eq!(
            *model.seen.lock().unwrap(),
            [1, 3],
            "the second call sees the tool result"
        );
        assert_eq!(host.log.lock().unwrap()[0].decision, Decision::Allowed);
    }

    #[tokio::test]
    async fn blocked_and_declined_tools_never_run() {
        let model = Script::new(vec![
            vec![
                call("a", "database_execute"),
                call("b", "database_describe_table"),
            ],
            vec![text("No pude hacerlo.")],
        ]);
        let tools = Fake::new(&[
            ("database_execute", Permission::Ask),
            ("database_describe_table", Permission::Deny),
        ]);
        let host = Recorder::default();
        host.answers.lock().unwrap().push_back(Approval::Reject);
        let mut history = vec![Message::text(Role::User, "Borra todo")];
        run_turn(&model, &tools, &host, "sys", 10_000, &mut history)
            .await
            .unwrap();

        assert!(tools.runs.lock().unwrap().is_empty());
        let decisions: Vec<Decision> = host
            .log
            .lock()
            .unwrap()
            .iter()
            .map(|a| a.decision)
            .collect();
        assert_eq!(decisions, [Decision::Rejected, Decision::Blocked]);
        let errors = history[2]
            .parts
            .iter()
            .filter(|p| matches!(p, Part::ToolResult { is_error: true, .. }))
            .count();
        assert_eq!(errors, 2);
        assert_eq!(
            kinds(&host),
            [
                "started", "approval", "finished", "started", "finished", "text"
            ]
        );
    }

    #[tokio::test]
    async fn always_allows_the_rest_of_the_turn_and_steps_are_capped() {
        let model = Script::new(vec![
            vec![call("a", "database_execute")],
            vec![call("b", "database_execute")],
            vec![text("Listo.")],
        ]);
        let tools = Fake::new(&[("database_execute", Permission::Ask)]);
        let host = Recorder::default();
        host.answers.lock().unwrap().push_back(Approval::Always);
        let mut history = vec![Message::text(Role::User, "Actualiza dos filas")];
        run_turn(&model, &tools, &host, "sys", 10_000, &mut history)
            .await
            .unwrap();
        assert_eq!(tools.runs.lock().unwrap().len(), 2);
        assert_eq!(kinds(&host).iter().filter(|k| **k == "approval").count(), 1);

        let endless = Script::new(
            (0..20)
                .map(|i| vec![call(&i.to_string(), "library_search")])
                .collect(),
        );
        let tools = Fake::new(&[("library_search", Permission::Allow)]);
        let mut history = vec![Message::text(Role::User, "?")];
        run_turn(
            &endless,
            &tools,
            &Recorder::default(),
            "sys",
            10_000,
            &mut history,
        )
        .await
        .unwrap();
        assert_eq!(tools.runs.lock().unwrap().len(), MAX_STEPS);
    }

    #[test]
    fn the_window_starts_at_a_question_and_shortens_old_output() {
        let result = |output: &str| Message {
            role: Role::User,
            parts: vec![Part::ToolResult {
                id: "x".into(),
                output: output.into(),
                is_error: false,
                view: None,
            }],
        };
        let history = vec![
            Message::text(Role::User, "primera"),
            Message {
                role: Role::Assistant,
                parts: vec![Part::ToolCall {
                    id: "x".into(),
                    name: "library_search".into(),
                    input: json!({}),
                }],
            },
            result(&"y".repeat(2000)),
            Message::text(Role::Assistant, "respuesta"),
            Message::text(Role::User, "segunda"),
        ];
        let all = window(&history, 100_000);
        assert_eq!(all.len(), 5);
        assert!(
            matches!(&all[2].parts[0], Part::ToolResult { output, .. } if output.chars().count() == OLD_OUTPUT_CHARS + 1)
        );
        // A budget that cuts inside the first turn starts at the next question instead.
        let tight = window(&history, 30);
        assert_eq!(tight, [Message::text(Role::User, "segunda")]);
    }

    /// A real conversation with a local model and the library tool.
    /// `MCFORGE_TEST_LIBRARY=dir MCFORGE_TEST_OLLAMA_MODEL=qwen2.5:7b-instruct cargo test real_chat -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs Ollama and a folder of documents"]
    async fn real_chat() {
        let (Ok(folder), Ok(model)) = (
            std::env::var("MCFORGE_TEST_LIBRARY"),
            std::env::var("MCFORGE_TEST_OLLAMA_MODEL"),
        ) else {
            panic!("set MCFORGE_TEST_LIBRARY and MCFORGE_TEST_OLLAMA_MODEL");
        };
        let dir = std::env::temp_dir().join(format!("mcforge-real-chat-{}", std::process::id()));
        let library = crate::library::Library::open(&dir.join("library.db"))
            .await
            .unwrap();
        library.set_folders(&[folder]).await.unwrap();
        library.update(None, |_| {}).await.unwrap();
        let catalog = crate::catalog::Catalog::load(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../catalog"),
        )
        .unwrap();
        let tools =
            tools::Builtin::new(&catalog, HashMap::new(), Some((&library, None)), None, None);
        let engine = crate::providers::chat::EngineModel::new(
            reqwest::Client::new(),
            crate::setup::Engine::Local { model },
        )
        .unwrap();
        let system = system_prompt("Spanish", "2026-10-31", &[]);
        let mut history = Vec::new();
        for question in [
            "¿Cuál es la meta de ventas de la región Norte y cuánto descuento puede dar un vendedor?",
            "¿Y cuántos días de vacaciones me tocan el primer año?",
        ] {
            history.push(Message::text(Role::User, question));
            let host = Recorder::default();
            let started = std::time::Instant::now();
            run_turn(&engine, &tools, &host, &system, 40_000, &mut history)
                .await
                .unwrap();
            let calls: Vec<String> = host
                .events
                .lock()
                .unwrap()
                .iter()
                .filter_map(|e| match e {
                    ChatEvent::ToolStarted { name, input, .. } => Some(format!("{name} {input}")),
                    _ => None,
                })
                .collect();
            let answer = history
                .last()
                .map(|m| format!("{:?}", m.parts))
                .unwrap_or_default();
            eprintln!(
                "\n{question}\n{:.1}s tools: {calls:?}\n{answer}",
                started.elapsed().as_secs_f64()
            );
        }
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn a_cancelled_call_gets_a_result() {
        let mut history = vec![
            Message::text(Role::User, "?"),
            Message {
                role: Role::Assistant,
                parts: vec![Part::ToolCall {
                    id: "x".into(),
                    name: "reports_render_report".into(),
                    input: json!({}),
                }],
            },
        ];
        repair(&mut history);
        assert!(
            matches!(&history[2].parts[0], Part::ToolResult { id, is_error: true, .. } if id == "x")
        );
        repair(&mut history);
        assert_eq!(history.len(), 3, "a valid conversation is left alone");
    }
}
