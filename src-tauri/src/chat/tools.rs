//! The tools a conversation can use: MCForge's own modules, offered only when
//! the user set them up. Whether a call runs is decided by `permission`, from
//! the user's rules, before `run` is ever reached.

use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::OnceCell;

use super::{ReportRef, SourceRef, Summary, Table};
use crate::catalog::Catalog;
use crate::database::{Database, Driver, QueryResult, WritableDatabase};
use crate::error::{Error, Result};
use crate::library::{Embedder, Library};
use crate::policy::Permission;
use crate::providers::chat::Model;
use crate::providers::stream::ToolSpec;
use crate::reports::focus::Focus;
use crate::reports::history::{History, ReportEntry};
use crate::reports::{self, Ai, NumberFormat, ReportRequest, Templates};
use crate::sources::{FileSource, display};

/// Rows of a result the model reads; the rest is summarized.
const MAX_ROWS: usize = 50;
/// Characters of any tool output the model reads.
const MAX_CHARS: usize = 6000;
const MAX_PASSAGES: usize = 10;
const PASSAGE_CHARS: usize = 900;

pub struct ToolOutput {
    /// What the model reads.
    pub text: String,
    pub summary: Summary,
    pub table: Option<Table>,
    pub sources: Vec<SourceRef>,
    pub report: Option<ReportRef>,
}

impl ToolOutput {
    fn text(text: String, summary: Summary) -> Self {
        Self {
            text,
            summary,
            table: None,
            sources: Vec::new(),
            report: None,
        }
    }
}

pub trait Tools: Send + Sync {
    fn specs(&self) -> Vec<ToolSpec>;
    /// The user's rule for a tool; anything unknown is blocked.
    fn permission(&self, name: &str) -> Permission;
    fn run<'a>(&'a self, name: &'a str, input: &'a Value) -> BoxFuture<'a, Result<ToolOutput>>;
}

/// A file the user attached to a conversation: the only files the report tool reads.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct Attachment {
    /// The file name, which is how the model refers to it.
    pub name: String,
    pub path: String,
    pub sheet: Option<String>,
}

/// What the report tool needs; present when the user set up the reports connection.
pub struct ReportTool<'a> {
    pub templates: &'a Templates,
    pub history: &'a History,
    pub cache: &'a Path,
    pub out_dir: PathBuf,
    pub model: &'a dyn Model,
    pub label: &'a str,
    pub attachments: &'a [Attachment],
    /// Presets with their names in the user's language, and the user's own.
    pub focuses: Vec<Focus>,
    pub language: String,
    pub number_format: NumberFormat,
}

/// MCForge's built-in modules as tools.
pub struct Builtin<'a> {
    pub catalog: &'a Catalog,
    pub permissions: HashMap<String, HashMap<String, Permission>>,
    /// Present when the library has documents.
    pub library: Option<(&'a Library, Option<&'a dyn Embedder>)>,
    pub database: Option<(Driver, String)>,
    pub reports: Option<ReportTool<'a>>,
    db: OnceCell<Database>,
}

impl<'a> Builtin<'a> {
    pub fn new(
        catalog: &'a Catalog,
        permissions: HashMap<String, HashMap<String, Permission>>,
        library: Option<(&'a Library, Option<&'a dyn Embedder>)>,
        database: Option<(Driver, String)>,
        reports: Option<ReportTool<'a>>,
    ) -> Self {
        Self {
            catalog,
            permissions,
            library,
            database,
            reports,
            db: OnceCell::new(),
        }
    }

    async fn db(&self) -> Result<&Database> {
        let (driver, url) = self
            .database
            .as_ref()
            .ok_or_else(|| Error::Database("no database is set up".into()))?;
        self.db
            .get_or_try_init(|| Database::open(*driver, url))
            .await
    }

    async fn search(&self, input: &Value) -> Result<ToolOutput> {
        let (library, embedder) = self
            .library
            .ok_or_else(|| Error::Io("the library is empty".into()))?;
        let query = text_input(input, "query")?;
        let limit = input["limit"]
            .as_u64()
            .map_or(6, |n| (n as usize).clamp(1, MAX_PASSAGES));
        let passages = library.search(embedder, query, limit).await?;
        let mut text = String::new();
        let mut sources: Vec<SourceRef> = Vec::new();
        for (i, p) in passages.iter().enumerate() {
            let passage: String = p.text.chars().take(PASSAGE_CHARS).collect();
            let _ = write!(text, "[{}] {} ({})\n{passage}\n\n", i + 1, p.title, p.path);
            if !sources.iter().any(|s| s.path == p.path) {
                sources.push(SourceRef {
                    title: p.title.clone(),
                    path: p.path.clone(),
                });
            }
        }
        if passages.is_empty() {
            text = "Nothing in the library matches. Try other words.".into();
        }
        Ok(ToolOutput {
            summary: Summary::Passages {
                count: passages.len() as u32,
                documents: sources.len() as u32,
            },
            sources,
            ..ToolOutput::text(clip(text), Summary::Skipped)
        })
    }

    async fn render_report(&self, tool: &ReportTool<'_>, input: &Value) -> Result<ToolOutput> {
        let name = text_input(input, "file")?;
        let attachment = tool
            .attachments
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| {
                Error::Report(format!("`{name}` is not attached to this conversation"))
            })?;
        let focus = input["focus"]
            .as_str()
            .and_then(|id| tool.focuses.iter().find(|f| f.id == id))
            .cloned();
        let template = tool
            .templates
            .list(&tool.language)
            .into_iter()
            .next()
            .ok_or_else(|| Error::Report("no report template is installed".into()))?;
        let request = ReportRequest {
            file: attachment.path.clone(),
            sheet: attachment.sheet.clone(),
            template: template.id,
            question: input["question"].as_str().map(str::to_owned),
            use_ai: true,
            focus,
            use_library: false,
            language: tool.language.clone(),
            number_format: tool.number_format.clone(),
        };
        let source = FileSource::import(
            Path::new(&request.file),
            request.sheet.as_deref(),
            tool.cache,
        )
        .await?;
        let ai = Ai {
            model: tool.model,
            label: tool.label,
        };
        let outcome = reports::generate(
            tool.templates,
            &source,
            Some(ai),
            &request,
            &[],
            &tool.out_dir,
            |_| {},
        )
        .await?;
        // The report exists either way; a history that cannot be saved only forgets it on restart.
        let _ = tool
            .history
            .add(ReportEntry::new(&outcome, source.summary.name()));
        Ok(ToolOutput {
            report: Some(ReportRef {
                path: outcome.path.clone(),
                title: outcome.title.clone(),
            }),
            ..ToolOutput::text(
                format!(
                    "Report created: \"{}\", {} sections, from {}. The user can open it from the conversation.",
                    outcome.title,
                    outcome.sections,
                    source.summary.name()
                ),
                Summary::Report {
                    sections: outcome.sections,
                },
            )
        })
    }
}

impl Tools for Builtin<'_> {
    fn specs(&self) -> Vec<ToolSpec> {
        let mut specs = Vec::new();
        let spec = |name: &str, description: &str, schema: Value| ToolSpec {
            name: name.into(),
            description: description.into(),
            schema,
        };
        let sql = json!({
            "type": "object",
            "properties": { "sql": { "type": "string", "description": "One SQL statement." } },
            "required": ["sql"],
        });
        if self.library.is_some() {
            specs.push(spec(
                "library_search",
                "Search the user's own documents (their library of PDFs, Word files and notes) by meaning and by words. \
                 Returns numbered passages with the document they come from. Use it whenever the question may be answered \
                 by the user's documents, and name the document when you use a passage.",
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "What to look for, in the user's words." },
                        "limit": { "type": "integer", "description": "Passages to return, 1 to 10. Default 6." },
                    },
                    "required": ["query"],
                }),
            ));
        }
        if let Some((driver, _)) = &self.database {
            let engine = format!("{driver:?}");
            specs.push(spec(
                "database_list_tables",
                &format!("List the tables of the user's {engine} database."),
                json!({ "type": "object", "properties": {} }),
            ));
            specs.push(spec(
                "database_describe_table",
                "List the columns of one table, with their types.",
                json!({
                    "type": "object",
                    "properties": { "table": { "type": "string" } },
                    "required": ["table"],
                }),
            ));
            specs.push(spec(
                "database_query",
                &format!(
                    "Run one read-only SELECT on the user's {engine} database. The database itself refuses any change. \
                     You see at most {MAX_ROWS} rows, so aggregate (COUNT, SUM, GROUP BY) instead of listing many rows."
                ),
                sql.clone(),
            ));
            specs.push(spec(
                "database_execute",
                "Change data in the user's database (INSERT, UPDATE, DELETE…). Only when the user explicitly asked for that change.",
                sql,
            ));
        }
        if let Some(tool) = &self.reports {
            specs.push(spec(
                "reports_list_templates",
                "List the report templates.",
                json!({ "type": "object", "properties": {} }),
            ));
            if !tool.attachments.is_empty() {
                let files: Vec<&str> = tool.attachments.iter().map(|a| a.name.as_str()).collect();
                let focuses: Vec<&str> = tool.focuses.iter().map(|f| f.id.as_str()).collect();
                let described: Vec<String> = tool
                    .focuses
                    .iter()
                    .map(|f| format!("{} ({})", f.id, f.name))
                    .collect();
                specs.push(spec(
                    "reports_render_report",
                    &format!(
                        "Create a report (a document with key figures, charts and written conclusions) from a data file the user \
                         attached. It takes a minute or two. Focus options: {}.",
                        described.join(", ")
                    ),
                    json!({
                        "type": "object",
                        "properties": {
                            "file": { "type": "string", "enum": files },
                            "question": { "type": "string", "description": "What the report should answer." },
                            "focus": { "type": "string", "enum": focuses, "description": "Who the report is for." },
                        },
                        "required": ["file"],
                    }),
                ));
            }
        }
        specs
    }

    fn permission(&self, name: &str) -> Permission {
        // Searching the user's own index is always allowed.
        if name == "library_search" {
            return if self.library.is_some() {
                Permission::Allow
            } else {
                Permission::Deny
            };
        }
        let offered: HashSet<String> = self.specs().into_iter().map(|s| s.name).collect();
        let Some((connector, tool)) = name.split_once('_').filter(|_| offered.contains(name))
        else {
            return Permission::Deny;
        };
        self.permissions
            .get(connector)
            .and_then(|rules| rules.get(tool))
            .copied()
            .or_else(|| {
                let connector = self.catalog.connector(connector).ok()?;
                connector
                    .tools
                    .iter()
                    .find(|t| t.name == tool)
                    .map(|t| t.default_permission)
            })
            .unwrap_or(Permission::Deny)
    }

    fn run<'b>(&'b self, name: &'b str, input: &'b Value) -> BoxFuture<'b, Result<ToolOutput>> {
        Box::pin(async move {
            match name {
                "library_search" => self.search(input).await,
                "database_list_tables" => {
                    let tables = self.db().await?.list_tables().await?;
                    Ok(ToolOutput::text(
                        clip(tables.join("\n")),
                        Summary::Tables {
                            count: tables.len() as u32,
                        },
                    ))
                }
                "database_describe_table" => {
                    let columns = self
                        .db()
                        .await?
                        .describe_table(text_input(input, "table")?)
                        .await?;
                    let rows: Vec<Vec<String>> = columns
                        .iter()
                        .map(|c| vec![c.name.clone(), c.data_type.clone(), c.nullable.to_string()])
                        .collect();
                    let table = Table {
                        columns: vec!["name".into(), "type".into(), "nullable".into()],
                        rows,
                    };
                    Ok(ToolOutput {
                        table: Some(table.clone()),
                        ..ToolOutput::text(
                            clip(table_text(&table, false)),
                            Summary::Columns {
                                count: columns.len() as u32,
                            },
                        )
                    })
                }
                "database_query" => {
                    let result = self.db().await?.query(text_input(input, "sql")?).await?;
                    Ok(query_output(&result))
                }
                "database_execute" => {
                    let (driver, url) = self
                        .database
                        .as_ref()
                        .ok_or_else(|| Error::Database("no database is set up".into()))?;
                    let changed = WritableDatabase::open(*driver, url)
                        .await?
                        .execute(text_input(input, "sql")?)
                        .await?;
                    Ok(ToolOutput::text(
                        format!("{changed} rows changed."),
                        Summary::Changed {
                            rows: u32::try_from(changed).unwrap_or(u32::MAX),
                        },
                    ))
                }
                "reports_list_templates" => {
                    let tool = self
                        .reports
                        .as_ref()
                        .ok_or_else(|| Error::Report("reports are not set up".into()))?;
                    let list = tool.templates.list(&tool.language);
                    let text = list
                        .iter()
                        .map(|t| format!("{}: {} — {}", t.id, t.name, t.description))
                        .collect::<Vec<_>>()
                        .join("\n");
                    Ok(ToolOutput::text(
                        text,
                        Summary::Templates {
                            count: list.len() as u32,
                        },
                    ))
                }
                "reports_render_report" => {
                    let tool = self
                        .reports
                        .as_ref()
                        .ok_or_else(|| Error::Report("reports are not set up".into()))?;
                    self.render_report(tool, input).await
                }
                other => Err(Error::Io(format!("unknown tool `{other}`"))),
            }
        })
    }
}

fn text_input<'v>(input: &'v Value, key: &str) -> Result<&'v str> {
    input[key]
        .as_str()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| Error::Io(format!("the tool needs `{key}`")))
}

fn query_output(result: &QueryResult) -> ToolOutput {
    let rows: Vec<Vec<String>> = result
        .rows
        .iter()
        .take(MAX_ROWS)
        .map(|r| r.iter().map(display).collect())
        .collect();
    let truncated = result.truncated || result.rows.len() > MAX_ROWS;
    let table = Table {
        columns: result.columns.clone(),
        rows,
    };
    let mut text = table_text(&table, truncated);
    if result.rows.is_empty() {
        text = "The query returned no rows.".into();
    }
    ToolOutput {
        table: Some(table),
        ..ToolOutput::text(
            clip(text),
            Summary::Rows {
                count: result.rows.len() as u32,
                truncated,
            },
        )
    }
}

/// Tab-separated, header first, with a note when rows were left out.
fn table_text(table: &Table, truncated: bool) -> String {
    let mut text = table.columns.join("\t");
    for row in &table.rows {
        text.push('\n');
        text.push_str(&row.join("\t"));
    }
    if truncated {
        let _ = write!(
            text,
            "\n(only the first {MAX_ROWS} rows are shown; aggregate to see everything)"
        );
    }
    text
}

/// Keeps tool output within what a small model's context can take.
fn clip(text: String) -> String {
    if text.chars().count() <= MAX_CHARS {
        return text;
    }
    let mut cut: String = text.chars().take(MAX_CHARS).collect();
    cut.push_str("\n(output cut short)");
    cut
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../catalog")).unwrap()
    }

    async fn sqlite(dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join("ventas.db");
        let url = file.to_string_lossy().into_owned();
        let w = WritableDatabase::open(Driver::Sqlite, &format!("sqlite://{url}?mode=rwc"))
            .await
            .unwrap();
        w.execute("CREATE TABLE ventas (region TEXT, total REAL)")
            .await
            .unwrap();
        w.execute("INSERT INTO ventas VALUES ('Norte', 10.5), ('Sur', 4)")
            .await
            .unwrap();
        url
    }

    #[tokio::test]
    async fn rules_decide_and_reads_stay_read_only() {
        let dir = std::env::temp_dir().join(format!("mcforge-chat-tools-{}", std::process::id()));
        let url = sqlite(&dir).await;
        let catalog = catalog();
        let rules = HashMap::from([(
            "database".to_owned(),
            HashMap::from([("describe_table".to_owned(), Permission::Deny)]),
        )]);
        let tools = Builtin::new(&catalog, rules, None, Some((Driver::Sqlite, url)), None);

        let names: Vec<String> = tools.specs().into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            [
                "database_list_tables",
                "database_describe_table",
                "database_query",
                "database_execute"
            ]
        );
        assert_eq!(
            tools.permission("database_query"),
            Permission::Allow,
            "catalog default for reads"
        );
        assert_eq!(
            tools.permission("database_execute"),
            Permission::Ask,
            "writes ask by default"
        );
        assert_eq!(
            tools.permission("database_describe_table"),
            Permission::Deny,
            "the user's rule wins"
        );
        assert_eq!(
            tools.permission("library_search"),
            Permission::Deny,
            "no library, no tool"
        );
        assert_eq!(tools.permission("shell_run"), Permission::Deny);
        assert_eq!(
            tools.permission("reports_render_report"),
            Permission::Deny,
            "not offered"
        );

        let out = tools
            .run(
                "database_query",
                &json!({ "sql": "SELECT region, total FROM ventas ORDER BY total DESC" }),
            )
            .await
            .unwrap();
        assert_eq!(
            out.summary,
            Summary::Rows {
                count: 2,
                truncated: false
            }
        );
        assert_eq!(out.text, "region\ttotal\nNorte\t10.5\nSur\t4.0");
        assert!(
            tools
                .run(
                    "database_query",
                    &json!({ "sql": "INSERT INTO ventas VALUES ('Centro', 1)" })
                )
                .await
                .is_err(),
            "the query tool cannot write"
        );
        let out = tools
            .run(
                "database_execute",
                &json!({ "sql": "INSERT INTO ventas VALUES ('Centro', 1)" }),
            )
            .await
            .unwrap();
        assert_eq!(out.summary, Summary::Changed { rows: 1 });
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn long_outputs_are_clipped() {
        let long = "x".repeat(MAX_CHARS + 10);
        assert!(clip(long).ends_with("(output cut short)"));
    }
}
