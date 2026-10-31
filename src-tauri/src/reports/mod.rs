//! Report engine: a template plus a data source becomes one self-contained HTML
//! file. Templates live in `templates/reports/<id>/` as a `template.toml`
//! manifest (names and labels per language) and a MiniJinja HTML file.

pub mod analysis;
pub mod chart;
pub mod history;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use minijinja::value::Serde;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use self::analysis::{ChartKind, Narrative, Section, Titles};
use self::chart::Point;
use crate::database::{Database, QueryResult};
use crate::error::{Error, Result};
use crate::providers::chat::Model;
use crate::sources::{self, ColumnKind, ColumnProfile, FileSource, display};

const PREVIEW_ROWS: usize = 10;
const DEFAULT_REQUEST: &str = "A general overview of the data: the main figures, the trend over time and breakdowns by category.";

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    file: String,
    locales: HashMap<String, Locale>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct Locale {
    name: String,
    description: String,
    #[serde(default)]
    labels: HashMap<String, String>,
}

impl Locale {
    fn label<'a>(&'a self, key: &'a str) -> &'a str {
        self.labels.get(key).map_or(key, String::as_str)
    }
}

struct Template {
    id: String,
    source: String,
    locales: HashMap<String, Locale>,
}

impl Template {
    /// The requested language (`es`, `es-MX`…), falling back to English.
    fn locale(&self, language: &str) -> &Locale {
        let base = language.split(['-', '_']).next().unwrap_or(language);
        self.locales
            .get(language)
            .or_else(|| self.locales.get(base))
            .or_else(|| self.locales.get("en"))
            .expect("validated on load")
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct TemplateInfo {
    pub id: String,
    pub name: String,
    pub description: String,
}

pub struct Templates(Vec<Template>);

impl Templates {
    pub fn load(dir: &Path) -> Result<Self> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| Error::Report(format!("{}: {e}", dir.display())))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.join("template.toml").is_file())
            .collect();
        dirs.sort();
        let templates = dirs
            .iter()
            .map(|d| {
                let read = |p: PathBuf| {
                    std::fs::read_to_string(&p)
                        .map_err(|e| Error::Report(format!("{}: {e}", p.display())))
                };
                let manifest: Manifest = toml::from_str(&read(d.join("template.toml"))?)
                    .map_err(|e| Error::Report(format!("{}: {e}", d.display())))?;
                if !manifest.locales.contains_key("en") {
                    return Err(Error::Report(format!(
                        "{}: an `en` locale is required",
                        d.display()
                    )));
                }
                let template = Template {
                    id: d
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                    source: read(d.join(&manifest.file))?,
                    locales: manifest.locales,
                };
                // Fail at startup, not when the user clicks "Generate".
                environment(&template)?;
                Ok(template)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self(templates))
    }

    pub fn list(&self, language: &str) -> Vec<TemplateInfo> {
        self.0
            .iter()
            .map(|t| {
                let l = t.locale(language);
                TemplateInfo {
                    id: t.id.clone(),
                    name: l.name.clone(),
                    description: l.description.clone(),
                }
            })
            .collect()
    }

    fn get(&self, id: &str) -> Result<&Template> {
        self.0
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| Error::Report(format!("unknown template `{id}`")))
    }
}

fn environment(template: &Template) -> Result<minijinja::Environment<'static>> {
    let mut env = minijinja::Environment::new();
    // The `.html` name turns on HTML escaping for every value.
    env.add_template_owned(format!("{}.html", template.id), template.source.clone())?;
    Ok(env)
}

/// How the user's system writes numbers, e.g. `1,234.5` or `1.234,5`.
#[derive(Debug, Clone, Deserialize, specta::Type)]
pub struct NumberFormat {
    pub decimal: String,
    pub group: String,
}

impl NumberFormat {
    pub fn format(&self, v: f64) -> String {
        let decimals = if (v.abs() * 100.0).round() % 100.0 == 0.0 {
            0
        } else {
            2
        };
        let text = format!("{:.*}", decimals, v.abs());
        let (int, frac) = text.split_once('.').unwrap_or((&text, ""));
        let mut out = String::with_capacity(text.len() + 4);
        if v < 0.0 && text.bytes().any(|b| b != b'0' && b != b'.') {
            out.push('-');
        }
        for (i, digit) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                out.push_str(&self.group);
            }
            out.push(digit);
        }
        if !frac.is_empty() {
            out.push_str(&self.decimal);
            out.push_str(frac);
        }
        out
    }
}

#[derive(Debug, Clone, Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ReportRequest {
    /// The CSV or spreadsheet file to report on.
    pub file: String,
    /// For a workbook; `None` picks its first sheet with data.
    pub sheet: Option<String>,
    pub template: String,
    pub question: Option<String>,
    pub use_ai: bool,
    pub language: String,
    pub number_format: NumberFormat,
}

#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ReportStage {
    Reading,
    Planning,
    Querying,
    Writing,
    Rendering,
}

/// Something that went wrong without stopping the report. The UI explains each kind.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "kind", content = "detail", rename_all = "camelCase")]
pub enum ReportWarning {
    PlanFailed(String),
    QueriesDropped(u32),
    NarrativeFailed(String),
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ReportOutcome {
    pub path: String,
    pub title: String,
    pub sections: u32,
    /// The model that wrote the plan or the conclusions, if one did.
    pub engine: Option<String>,
    pub warnings: Vec<ReportWarning>,
}

/// The AI engine for one report and the name it is shown with.
pub struct Ai<'a> {
    pub model: &'a dyn Model,
    pub label: &'a str,
}

pub async fn generate(
    templates: &Templates,
    source: &FileSource,
    ai: Option<Ai<'_>>,
    request: &ReportRequest,
    out_dir: &Path,
    on_stage: impl Fn(ReportStage) + Send + Sync,
) -> Result<ReportOutcome> {
    let template = templates.get(&request.template)?;
    let locale = template.locale(&request.language);
    let summary = &source.summary;
    let db = source.open().await?;
    let profile = sources::profile(&db, summary).await?;
    let preview = sources::sample(&db, summary, PREVIEW_ROWS).await?;
    let question = request
        .question
        .as_deref()
        .map(str::trim)
        .filter(|q| !q.is_empty());
    let ask = question.unwrap_or(DEFAULT_REQUEST);
    let language = language_name(&request.language);
    let data = analysis::describe(summary, &profile, &preview);

    let mut warnings = Vec::new();
    let mut ai_used = false;
    let mut title = String::new();
    let mut sections = Vec::new();
    if let Some(ai) = &ai {
        on_stage(ReportStage::Planning);
        match plan_with_ai(ai.model, &db, &data, ask, language, &on_stage).await {
            Ok((t, s, dropped)) => {
                (title, sections, ai_used) = (t, s, true);
                if dropped > 0 {
                    warnings.push(ReportWarning::QueriesDropped(dropped));
                }
            }
            Err(e) => warnings.push(ReportWarning::PlanFailed(e.to_string())),
        }
    }
    if sections.is_empty() {
        on_stage(ReportStage::Querying);
        let titles = Titles {
            by_month: locale.label("by_month"),
            count_by_month: locale.label("count_by_month"),
            by_dim: locale.label("by_dim"),
            count_by_dim: locale.label("count_by_dim"),
            top_rows: locale.label("top_rows"),
        };
        sections = analysis::run(&db, analysis::auto_plan(summary, &profile, &titles))
            .await
            .0;
    }

    let mut narrative = Narrative::default();
    if let Some(ai) = &ai
        && !sections.is_empty()
    {
        on_stage(ReportStage::Writing);
        let fmt = |v: f64| request.number_format.format(v);
        match analysis::ai_narrative(ai.model, &data, ask, language, &fmt, &mut sections).await {
            Ok(n) => (narrative, ai_used) = (n, true),
            Err(e) => warnings.push(ReportWarning::NarrativeFailed(e.to_string())),
        }
    }

    on_stage(ReportStage::Rendering);
    if title.is_empty() {
        title = format!("{} · {}", locale.name, summary.name());
    }
    let engine = ai
        .as_ref()
        .filter(|_| ai_used)
        .map(|ai| ai.label.to_owned());
    let html = render(
        template,
        locale,
        &Page {
            language: &request.language,
            title: &title,
            question,
            engine: engine.as_deref(),
            profile: &profile,
            preview: &preview,
            sections: &sections,
            narrative: &narrative,
            source,
            number_format: &request.number_format,
        },
    )?;

    std::fs::create_dir_all(out_dir)?;
    let slug: String = sources::identifier(&title, "report")
        .replace('_', "-")
        .chars()
        .take(60)
        .collect();
    let path = out_dir.join(format!(
        "{}-{}.html",
        slug.trim_end_matches('-'),
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::write(&path, html)?;
    Ok(ReportOutcome {
        path: path.to_string_lossy().into_owned(),
        title,
        sections: u32::try_from(sections.len()).unwrap_or(u32::MAX),
        engine,
        warnings,
    })
}

/// The AI's plan, with one chance to fix the queries that failed.
async fn plan_with_ai(
    model: &dyn Model,
    db: &Database,
    data: &str,
    ask: &str,
    language: &str,
    on_stage: &(impl Fn(ReportStage) + Send + Sync),
) -> Result<(String, Vec<Section>, u32)> {
    let (title, plans) = analysis::ai_plan(model, data, ask, language, &[]).await?;
    on_stage(ReportStage::Querying);
    let (mut sections, mut failures) = analysis::run(db, plans).await;
    if !failures.is_empty() {
        on_stage(ReportStage::Planning);
        if let Ok((_, retry)) = analysis::ai_plan(model, data, ask, language, &failures).await {
            on_stage(ReportStage::Querying);
            let (again, still) = analysis::run(db, retry).await;
            if again.len() > sections.len() {
                (sections, failures) = (again, still);
            }
        }
    }
    if sections.is_empty() {
        let reason = failures
            .first()
            .map(|(_, e)| e.as_str())
            .unwrap_or("no sections");
        return Err(Error::Report(format!(
            "none of the AI's queries worked ({reason})"
        )));
    }
    Ok((
        title,
        sections,
        u32::try_from(failures.len()).unwrap_or(u32::MAX),
    ))
}

fn language_name(code: &str) -> &'static str {
    match code.split(['-', '_']).next().unwrap_or(code) {
        "es" => "Spanish",
        "pt" => "Portuguese",
        "fr" => "French",
        "de" => "German",
        "it" => "Italian",
        _ => "English",
    }
}

struct Page<'a> {
    language: &'a str,
    title: &'a str,
    question: Option<&'a str>,
    engine: Option<&'a str>,
    profile: &'a [ColumnProfile],
    preview: &'a QueryResult,
    sections: &'a [Section],
    narrative: &'a Narrative,
    source: &'a FileSource,
    number_format: &'a NumberFormat,
}

#[derive(Serialize)]
struct Context<'a> {
    lang: &'a str,
    t: &'a HashMap<String, String>,
    title: &'a str,
    question: Option<&'a str>,
    engine: Option<&'a str>,
    file: String,
    generated_at: String,
    summary: &'a [String],
    highlights: &'a [String],
    figures: Vec<Figure>,
    columns: Vec<ColumnView>,
    sections: Vec<SectionView>,
    preview: TableView,
}

#[derive(Serialize)]
struct Figure {
    label: String,
    value: String,
}

#[derive(Serialize)]
struct ColumnView {
    label: String,
    kind: ColumnKind,
    filled: String,
    distinct: String,
    values: String,
}

#[derive(Serialize)]
struct SectionView {
    title: String,
    commentary: Option<String>,
    /// Inline SVG built by `chart`, which escapes every label.
    chart: Option<String>,
    table: Option<TableView>,
}

#[derive(Serialize)]
struct TableView {
    columns: Vec<String>,
    numeric: Vec<bool>,
    rows: Vec<Vec<String>>,
    more: usize,
}

fn render(template: &Template, locale: &Locale, page: &Page) -> Result<String> {
    let fmt = page.number_format;
    let summary = &page.source.summary;
    let rows = u64::from(summary.rows);
    let mut figures = vec![
        Figure {
            label: locale.label("rows").into(),
            value: fmt.format(rows as f64),
        },
        Figure {
            label: locale.label("columns").into(),
            value: summary.columns.len().to_string(),
        },
    ];
    if let Some(date) = page
        .profile
        .iter()
        .find(|c| c.kind == ColumnKind::Date && !c.min.is_null())
    {
        figures.push(Figure {
            label: locale.label("period").into(),
            value: format!("{} – {}", day(&date.min), day(&date.max)),
        });
    }
    if let Some(m) = analysis::primary_measure(page.profile, summary.rows)
        && let Some(sum) = m.sum
    {
        figures.push(Figure {
            label: locale.label("total").replace("{measure}", &m.label),
            value: fmt.format(sum),
        });
    }

    let columns = page
        .profile
        .iter()
        .map(|c| ColumnView {
            label: c.label.clone(),
            kind: c.kind,
            filled: format!("{}%", (c.filled * 100).checked_div(rows).unwrap_or(0)),
            distinct: fmt.format(c.distinct as f64),
            values: match c.kind {
                ColumnKind::Text => c
                    .top
                    .iter()
                    .map(|(v, _)| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                ColumnKind::Date => format!("{} – {}", day(&c.min), day(&c.max)),
                _ => format!(
                    "{} – {}",
                    cell(&c.min, fmt, false),
                    cell(&c.max, fmt, false)
                ),
            },
        })
        .collect();

    let number = |v: f64| fmt.format(v);
    let sections = page
        .sections
        .iter()
        .map(|s| {
            let points = || -> Vec<Point> {
                s.result
                    .rows
                    .iter()
                    .filter_map(|r| {
                        Some(Point {
                            label: display(r.first()?),
                            value: r.get(1)?.as_f64()?,
                        })
                    })
                    .collect()
            };
            let (chart, table) = match s.chart {
                ChartKind::Bar => (Some(chart::bar(&points(), &number)), None),
                ChartKind::Line => (Some(chart::line(&points(), &number)), None),
                ChartKind::Table => (None, Some(table(&s.result, analysis::MAX_TABLE_ROWS, fmt))),
            };
            SectionView {
                title: s.title.clone(),
                commentary: s.commentary.clone(),
                chart,
                table,
            }
        })
        .collect();

    let context = Context {
        lang: page.language,
        t: &locale.labels,
        title: page.title,
        question: page.question,
        engine: page.engine,
        file: summary.name(),
        generated_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        summary: &page.narrative.summary,
        highlights: &page.narrative.highlights,
        figures,
        columns,
        sections,
        preview: table(page.preview, PREVIEW_ROWS, fmt),
    };
    let env = environment(template)?;
    Ok(env
        .get_template(&format!("{}.html", template.id))?
        .render(Serde(&context))?)
}

fn table(r: &QueryResult, max_rows: usize, fmt: &NumberFormat) -> TableView {
    let numeric: Vec<bool> = (0..r.columns.len())
        .map(|i| {
            r.rows
                .iter()
                .all(|row| row.get(i).is_some_and(|v| v.is_number() || v.is_null()))
                && r.rows
                    .iter()
                    .any(|row| row.get(i).is_some_and(Value::is_number))
        })
        .collect();
    let plain: Vec<bool> = r.columns.iter().map(|c| is_code_column(c)).collect();
    TableView {
        rows: r
            .rows
            .iter()
            .take(max_rows)
            .map(|row| {
                row.iter()
                    .enumerate()
                    .map(|(i, v)| cell(v, fmt, plain[i]))
                    .collect()
            })
            .collect(),
        more: r.rows.len().saturating_sub(max_rows),
        columns: r.columns.clone(),
        numeric,
    }
}

/// Identifiers and years read better without thousands separators.
fn is_code_column(name: &str) -> bool {
    let name = sources::identifier(name, "");
    name.split('_')
        .any(|p| matches!(p, "id" | "year" | "ano" | "code" | "codigo" | "zip" | "cp"))
}

fn cell(v: &Value, fmt: &NumberFormat, plain: bool) -> String {
    match v {
        Value::Number(n) if !plain => n
            .as_f64()
            .map(|f| fmt.format(f))
            .unwrap_or_else(|| n.to_string()),
        Value::Bool(b) => if *b { "✓" } else { "✗" }.into(),
        other => display(other),
    }
}

/// Dates without a midnight time part.
fn day(v: &Value) -> String {
    display(v).trim_end_matches(" 00:00:00").to_owned()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use futures_util::future::BoxFuture;
    use serde_json::json;

    use super::*;
    use crate::providers::chat::EngineModel;

    /// A model that replays canned answers, so the whole pipeline runs offline.
    struct Scripted(Mutex<VecDeque<Result<Value>>>);

    impl Model for Scripted {
        fn json<'a>(
            &'a self,
            _: &'a str,
            _: &'a str,
            _: &'a Value,
        ) -> BoxFuture<'a, Result<Value>> {
            let next = self
                .0
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected model call");
            Box::pin(async move { next })
        }
    }

    fn templates() -> Templates {
        Templates::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../templates/reports"))
            .unwrap()
    }

    fn request(use_ai: bool, question: Option<&str>) -> ReportRequest {
        ReportRequest {
            file: String::new(),
            sheet: None,
            template: "data-overview".into(),
            question: question.map(str::to_owned),
            use_ai,
            language: "es".into(),
            number_format: NumberFormat {
                decimal: ".".into(),
                group: ",".into(),
            },
        }
    }

    async fn sales(dir: &Path) -> FileSource {
        std::fs::create_dir_all(dir).unwrap();
        let file = dir.join("ventas.csv");
        std::fs::write(
            &file,
            "Fecha,Región,Producto,Total\n\
             2026-01-05,Norte,Laptop,1200.50\n2026-01-20,Sur,Mouse,25\n\
             2026-02-03,Norte,Monitor,300\n2026-02-14,Centro,Laptop,1150\n\
             2026-03-01,Sur,Laptop,1300\n2026-03-09,Norte,<b>Teclado</b>,45.5\n",
        )
        .unwrap();
        FileSource::import(&file, None, dir).await.unwrap()
    }

    #[test]
    fn shipped_templates_load_and_translate_every_label() {
        let templates = templates();
        assert!(!templates.0.is_empty());
        for t in &templates.0 {
            let en: Vec<_> = t.locales["en"].labels.keys().collect();
            for (code, locale) in &t.locales {
                let mut keys: Vec<_> = locale.labels.keys().collect();
                let mut want = en.clone();
                keys.sort();
                want.sort();
                assert_eq!(keys, want, "{}: `{code}` labels differ from `en`", t.id);
            }
        }
        assert_eq!(templates.list("es-MX")[0].name, "Resumen de datos");
    }

    #[test]
    fn numbers_follow_the_users_locale() {
        let es = NumberFormat {
            decimal: ",".into(),
            group: ".".into(),
        };
        assert_eq!(es.format(1234567.891), "1.234.567,89");
        assert_eq!(es.format(-1200.0), "-1.200");
        assert_eq!(es.format(-0.001), "0");
        let en = NumberFormat {
            decimal: ".".into(),
            group: ",".into(),
        };
        assert_eq!(en.format(999.5), "999.50");
    }

    #[tokio::test]
    async fn without_ai_the_rules_build_a_complete_report() {
        let dir = std::env::temp_dir().join(format!("mcforge-report-rules-{}", std::process::id()));
        let source = sales(&dir).await;
        let outcome = generate(
            &templates(),
            &source,
            None,
            &request(false, None),
            &dir,
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(outcome.engine, None);
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
        let html = std::fs::read_to_string(&outcome.path).unwrap();
        assert!(html.contains("Total por mes"), "monthly trend section");
        assert!(
            html.contains("Total por Región"),
            "breakdown by the original header"
        );
        assert!(html.contains("4,021"), "total of the amount column");
        assert!(html.contains("&lt;b&gt;Teclado"), "data is escaped");
        assert!(!html.contains("<b>Teclado"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_workbook_sheet_reports_like_a_csv() {
        let dir = std::env::temp_dir().join(format!("mcforge-report-xlsx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Ventas 2026.xlsx");
        crate::sources::tests::sales_workbook(&file);
        let source = FileSource::import(&file, None, &dir).await.unwrap();
        let outcome = generate(
            &templates(),
            &source,
            None,
            &request(false, None),
            &dir,
            |_| {},
        )
        .await
        .unwrap();
        let html = std::fs::read_to_string(&outcome.path).unwrap();
        assert!(
            html.contains("Ventas 2026.xlsx · Ventas"),
            "source names the sheet"
        );
        assert!(html.contains("Total por mes"));
        assert!(html.contains("1,829.75"), "total of the amount column");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn ai_plans_are_run_read_only_and_repaired_once() {
        let dir = std::env::temp_dir().join(format!("mcforge-report-ai-{}", std::process::id()));
        let source = sales(&dir).await;
        let model = Scripted(Mutex::new(VecDeque::from([
            Ok(json!({ "title": "Ventas del trimestre", "sections": [
                { "title": "Por producto", "sql": "SELECT producto, SUM(total) FROM ventas GROUP BY 1 ORDER BY 2 DESC", "chart": "bar" },
                { "title": "Borrar", "sql": "DELETE FROM ventas", "chart": "table" },
                { "title": "Roto", "sql": "SELECT nope FROM ventas", "chart": "table" },
            ]})),
            Ok(json!({ "title": "x", "sections": [
                { "title": "Por producto", "sql": "SELECT producto, SUM(total) FROM ventas GROUP BY 1 ORDER BY 2 DESC", "chart": "bar" },
                { "title": "Por región", "sql": "SELECT region, COUNT(*) AS pedidos FROM ventas GROUP BY 1", "chart": "bar" },
            ]})),
            Ok(json!({
                "summary": "Las laptops dominan.\n\nEl norte compra más.",
                "highlights": ["- Laptop: 3,650.50"],
                "commentaries": ["Laptop encabeza.", "Norte lidera."],
            })),
        ])));
        let stages = Mutex::new(Vec::new());
        let ai = Ai {
            model: &model,
            label: "Modelo de prueba",
        };
        let outcome = generate(
            &templates(),
            &source,
            Some(ai),
            &request(true, Some("¿Qué se vende más?")),
            &dir,
            |s| stages.lock().unwrap().push(format!("{s:?}")),
        )
        .await
        .unwrap();

        assert_eq!(outcome.title, "Ventas del trimestre");
        assert_eq!(outcome.sections, 2);
        assert_eq!(outcome.engine.as_deref(), Some("Modelo de prueba"));
        assert!(
            outcome.warnings.is_empty(),
            "the repaired plan dropped nothing: {:?}",
            outcome.warnings
        );
        assert_eq!(
            stages.into_inner().unwrap(),
            [
                "Planning",
                "Querying",
                "Planning",
                "Querying",
                "Writing",
                "Rendering"
            ]
        );
        let html = std::fs::read_to_string(&outcome.path).unwrap();
        for text in [
            "Las laptops dominan.",
            "El norte compra más.",
            "Laptop: 3,650.50",
            "Norte lidera.",
            "¿Qué se vende más?",
        ] {
            assert!(html.contains(text), "missing {text}");
        }
        let db = source.open().await.unwrap();
        let rows = db.query("SELECT COUNT(*) FROM ventas").await.unwrap();
        assert_eq!(rows.rows[0][0], json!(6), "the DELETE never ran");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_failing_model_still_yields_a_report() {
        let dir = std::env::temp_dir().join(format!("mcforge-report-down-{}", std::process::id()));
        let source = sales(&dir).await;
        let model = Scripted(Mutex::new(VecDeque::from([
            Err(Error::Provider("Ollama is not running.".into())),
            Err(Error::Provider("Ollama is not running.".into())),
        ])));
        let ai = Ai {
            model: &model,
            label: "x",
        };
        let outcome = generate(
            &templates(),
            &source,
            Some(ai),
            &request(true, None),
            &dir,
            |_| {},
        )
        .await
        .unwrap();
        assert_eq!(outcome.engine, None);
        assert!(outcome.sections > 0);
        assert!(matches!(
            outcome.warnings[..],
            [
                ReportWarning::PlanFailed(_),
                ReportWarning::NarrativeFailed(_)
            ]
        ));
        std::fs::remove_dir_all(dir).ok();
    }

    /// The whole pipeline against a real local model, for tuning prompts:
    /// `MCFORGE_TEST_OLLAMA_MODEL=qwen3:8b MCFORGE_TEST_CSV=path/to.csv cargo test real_local_model -- --ignored --nocapture`
    #[tokio::test]
    #[ignore = "needs Ollama, a model and a CSV file"]
    async fn real_local_model() {
        let (Ok(model), Ok(csv)) = (
            std::env::var("MCFORGE_TEST_OLLAMA_MODEL"),
            std::env::var("MCFORGE_TEST_CSV"),
        ) else {
            panic!("set MCFORGE_TEST_OLLAMA_MODEL and MCFORGE_TEST_CSV");
        };
        let dir = std::env::temp_dir().join("mcforge-real-model");
        let source = FileSource::import(Path::new(&csv), None, &dir)
            .await
            .unwrap();
        let engine = EngineModel::new(
            reqwest::Client::new(),
            crate::setup::Engine::Local {
                model: model.clone(),
            },
        )
        .unwrap();
        let mut req = request(true, std::env::var("MCFORGE_TEST_QUESTION").ok().as_deref());
        req.file = csv;
        let started = std::time::Instant::now();
        let outcome = generate(
            &templates(),
            &source,
            Some(Ai {
                model: &engine,
                label: &model,
            }),
            &req,
            &dir,
            |s| {
                eprintln!("{:>6.1}s {s:?}", started.elapsed().as_secs_f64());
            },
        )
        .await
        .unwrap();
        eprintln!("{outcome:#?}");
    }
}
