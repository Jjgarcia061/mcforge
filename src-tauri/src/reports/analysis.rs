//! What goes in a report. The AI picks sections from the data's shape and the
//! user's question, or simple rules do when there is no AI. The AI only ever
//! writes SELECT queries, which run read-only, and prose; every number in the
//! report comes from the data.

use std::fmt::Write;

use serde::Deserialize;
use serde_json::{Value, json};

use super::focus::Focus;
use crate::database::{Database, QueryResult};
use crate::error::{Error, Result};
use crate::providers::chat::Model;
use crate::sources::{ColumnKind, ColumnProfile, SourceSummary, display};

const MAX_SECTIONS: usize = 8;
pub const MAX_TABLE_ROWS: usize = 20;
/// Rows of each result the AI sees when writing conclusions.
const NARRATIVE_ROWS: usize = 25;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChartKind {
    Bar,
    Line,
    Table,
}

impl ChartKind {
    fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "bar" => Self::Bar,
            "line" => Self::Line,
            _ => Self::Table,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SectionPlan {
    pub title: String,
    pub sql: String,
    pub chart: ChartKind,
}

pub struct Section {
    pub title: String,
    pub chart: ChartKind,
    pub result: QueryResult,
    pub commentary: Option<String>,
}

#[derive(Default)]
pub struct Narrative {
    pub summary: Vec<String>,
    pub highlights: Vec<String>,
}

/// Localised section titles for the rule-based plan, from the template's labels.
pub struct Titles<'a> {
    pub by_month: &'a str,
    pub count_by_month: &'a str,
    pub by_dim: &'a str,
    pub count_by_dim: &'a str,
    pub top_rows: &'a str,
}

/// The column most likely to be "the amount": numeric, not an identifier, money-like names first.
pub fn primary_measure(profile: &[ColumnProfile], rows: u32) -> Option<&ColumnProfile> {
    const ID_LIKE: [&str; 9] = [
        "id", "codigo", "code", "zip", "cp", "telefono", "phone", "year", "ano",
    ];
    const MONEY: [&str; 12] = [
        "total", "importe", "monto", "amount", "revenue", "venta", "sales", "ingreso", "precio",
        "price", "costo", "cost",
    ];
    let measures: Vec<_> = profile
        .iter()
        .filter(|c| c.kind.is_number())
        .filter(|c| !c.name.split('_').any(|part| ID_LIKE.contains(&part)))
        .filter(|c| !(c.kind == ColumnKind::Integer && rows > 1 && c.distinct == u64::from(rows)))
        .collect();
    MONEY
        .iter()
        .find_map(|m| measures.iter().find(|c| c.name.contains(m)))
        .or(measures.first())
        .copied()
}

fn dimensions(profile: &[ColumnProfile], rows: u32) -> impl Iterator<Item = &ColumnProfile> {
    profile.iter().filter(move |c| {
        c.kind == ColumnKind::Text && (2..=25).contains(&c.distinct) && c.distinct < u64::from(rows)
    })
}

/// Sections chosen by rules: a trend over time, then breakdowns by the main categories.
pub fn auto_plan(
    summary: &SourceSummary,
    profile: &[ColumnProfile],
    titles: &Titles,
) -> Vec<SectionPlan> {
    let t = &summary.table;
    let measure = primary_measure(profile, summary.rows);
    let (value, value_label) = match measure {
        Some(m) => (format!("ROUND(SUM(\"{}\"), 2)", m.name), m.label.as_str()),
        None => ("COUNT(*)".into(), ""),
    };
    let fill = |pattern: &str, dim: &str| {
        pattern
            .replace("{measure}", value_label)
            .replace("{dim}", dim)
    };
    let mut plans = Vec::new();

    if let Some(date) = profile.iter().find(|c| c.kind == ColumnKind::Date) {
        plans.push(SectionPlan {
            title: fill(if measure.is_some() { titles.by_month } else { titles.count_by_month }, ""),
            sql: format!(
                "SELECT strftime('%Y-%m', \"{d}\") AS period, {value} AS value FROM \"{t}\" WHERE \"{d}\" IS NOT NULL GROUP BY 1 ORDER BY 1",
                d = date.name
            ),
            chart: ChartKind::Line,
        });
    }
    for dim in dimensions(profile, summary.rows).take(3) {
        plans.push(SectionPlan {
            title: fill(if measure.is_some() { titles.by_dim } else { titles.count_by_dim }, &dim.label),
            sql: format!(
                "SELECT \"{d}\" AS label, {value} AS value FROM \"{t}\" GROUP BY 1 ORDER BY 2 DESC LIMIT 12",
                d = dim.name
            ),
            chart: ChartKind::Bar,
        });
    }
    if plans.is_empty()
        && let Some(m) = measure
    {
        plans.push(SectionPlan {
            title: fill(titles.top_rows, ""),
            sql: format!(
                "SELECT * FROM \"{t}\" ORDER BY \"{}\" DESC LIMIT 10",
                m.name
            ),
            chart: ChartKind::Table,
        });
    }
    plans
}

/// The table as the AI sees it: columns with their types and value ranges, plus a few rows.
pub fn describe(
    summary: &SourceSummary,
    profile: &[ColumnProfile],
    sample: &QueryResult,
) -> String {
    let mut out = format!(
        "Table \"{}\" ({} rows). Columns:\n",
        summary.table, summary.rows
    );
    for c in profile {
        let kind = match c.kind {
            ColumnKind::Integer => "integer",
            ColumnKind::Decimal => "decimal",
            ColumnKind::Date => "date",
            ColumnKind::Text => "text",
        };
        let _ = write!(out, "- {} ({kind}", c.name);
        if c.label != c.name {
            let _ = write!(out, ", header \"{}\"", c.label);
        }
        let _ = write!(out, ", {} filled, {} distinct", c.filled, c.distinct);
        match c.kind {
            ColumnKind::Text if !c.top.is_empty() => {
                let top: Vec<_> = c.top.iter().map(|(v, n)| format!("{v} ({n})")).collect();
                let _ = write!(out, "; most common: {}", top.join(", "));
            }
            ColumnKind::Text => {}
            _ => {
                let _ = write!(out, "; from {} to {}", display(&c.min), display(&c.max));
                if let Some(sum) = c.sum {
                    let _ = write!(out, "; sum {sum:.2}");
                }
            }
        }
        out.push_str(")\n");
    }
    out.push_str("\nFirst rows:\n");
    out.push_str(&table_text(sample, 5, None));
    out
}

fn table_text(r: &QueryResult, max_rows: usize, fmt: Option<&dyn Fn(f64) -> String>) -> String {
    let mut out = r.columns.join(" | ");
    out.push('\n');
    for row in r.rows.iter().take(max_rows) {
        let cells: Vec<String> = row
            .iter()
            .map(|v| match (v.as_f64(), fmt) {
                (Some(n), Some(fmt)) => fmt(n),
                _ => display(v),
            })
            .collect();
        out.push_str(&cells.join(" | "));
        out.push('\n');
    }
    if r.rows.len() > max_rows {
        let _ = writeln!(out, "… ({} more rows)", r.rows.len() - max_rows);
    }
    out
}

/// What the reader asked for: the request, who the report is for and its language.
pub struct Brief<'a> {
    pub request: &'a str,
    /// The language's English name, as the model reads it.
    pub language: &'a str,
    pub focus: Option<&'a Focus>,
}

impl Brief<'_> {
    /// The start of every prompt after the data: what to answer and for whom.
    fn head(&self) -> String {
        let mut head = format!("Request: {}\n", self.request);
        if let Some(focus) = self.focus.filter(|f| !f.instructions.trim().is_empty()) {
            let _ = writeln!(
                head,
                "Report focus ({}): {}\nChoose the analyses and write the conclusions for this reader.",
                focus.name.trim(),
                focus.instructions.trim()
            );
        }
        head
    }
}

const SYSTEM: &str = "You are a careful data analyst preparing a business report. \
You can only read the data with SQLite SELECT queries over the table described. \
Never invent columns, values or figures.";

#[derive(Deserialize)]
struct PlanReply {
    #[serde(default)]
    title: String,
    #[serde(default)]
    sections: Vec<PlanItem>,
}

#[derive(Deserialize)]
struct PlanItem {
    #[serde(default)]
    title: String,
    #[serde(default)]
    sql: String,
    #[serde(default)]
    chart: String,
}

fn plan_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["title", "sections"],
        "properties": {
            "title": { "type": "string" },
            "sections": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["title", "sql", "chart"],
                    "properties": {
                        "title": { "type": "string" },
                        "sql": { "type": "string" },
                        "chart": { "type": "string", "enum": ["bar", "line", "table"] },
                    },
                },
            },
        },
    })
}

/// Asks the AI for a title and sections. `failures` carries queries that failed last time.
pub async fn ai_plan(
    model: &dyn Model,
    data: &str,
    brief: &Brief<'_>,
    failures: &[(SectionPlan, String)],
) -> Result<(String, Vec<SectionPlan>)> {
    let (head, language) = (brief.head(), brief.language);
    let mut prompt = format!(
        "{data}\n{head}\n\
         Plan a report that answers the request with 3 to 6 sections. For each section give a short title, \
         one SQLite query and a chart type:\n\
         - \"bar\": the query returns exactly 2 columns (label, number), at most 12 rows, largest first.\n\
         - \"line\": the query returns exactly 2 columns (period, number) ordered by period; \
         group dates by month with strftime('%Y-%m', column).\n\
         - \"table\": any columns, at most 20 rows.\n\
         Rules: SELECT only; use only the table and columns listed; dates are text 'YYYY-MM-DD'; \
         round amounts with ROUND(x, 2); give columns short readable aliases.\n\
         Also give the whole report a short title. Write titles in {language}.\n"
    );
    if !failures.is_empty() {
        prompt.push_str("\nThese queries from your previous plan failed. Return the whole plan again with them fixed:\n");
        for (plan, error) in failures {
            let _ = writeln!(prompt, "- {}: {} -> error: {error}", plan.title, plan.sql);
        }
    }
    let reply: PlanReply =
        serde_json::from_value(model.json(SYSTEM, &prompt, &plan_schema()).await?)
            .map_err(|e| Error::Provider(format!("The model's plan was malformed: {e}")))?;
    let sections = reply
        .sections
        .into_iter()
        .filter(|s| !s.sql.trim().is_empty())
        .take(MAX_SECTIONS)
        .map(|s| SectionPlan {
            title: s.title.trim().to_owned(),
            sql: s.sql,
            chart: ChartKind::parse(&s.chart),
        })
        .collect();
    Ok((reply.title.trim().to_owned(), sections))
}

/// Runs each plan through the read-only database. Returns the sections that
/// produced data and, separately, the plans that failed with their error.
pub async fn run(
    db: &Database,
    plans: Vec<SectionPlan>,
) -> (Vec<Section>, Vec<(SectionPlan, String)>) {
    let mut sections = Vec::new();
    let mut failures = Vec::new();
    for plan in plans {
        let sql = plan.sql.trim().trim_end_matches(';');
        let lower = sql.to_ascii_lowercase();
        if !(lower.starts_with("select") || lower.starts_with("with")) {
            failures.push((plan, "only SELECT queries are allowed".into()));
            continue;
        }
        match db.query(sql).await {
            Ok(result) if result.rows.is_empty() => {
                failures.push((plan, "the query returned no rows".into()))
            }
            Ok(result) => {
                let chart = fit_chart(plan.chart, &result);
                // A long table cannot be read in a report, nor fully seen by the AI that explains it.
                if chart == ChartKind::Table && result.rows.len() > MAX_TABLE_ROWS {
                    let error = format!(
                        "it returned {} rows; a table may have at most {MAX_TABLE_ROWS} (aggregate or add LIMIT)",
                        result.rows.len()
                    );
                    failures.push((plan, error));
                    continue;
                }
                sections.push(Section {
                    chart,
                    title: plan.title,
                    result,
                    commentary: None,
                });
            }
            Err(e) => failures.push((plan, e.to_string())),
        }
    }
    (sections, failures)
}

/// Highs, lows, shares and change, computed so the AI never has to do arithmetic.
fn facts(s: &Section, fmt: &dyn Fn(f64) -> String) -> Option<String> {
    if s.chart == ChartKind::Table {
        return None;
    }
    let points: Vec<(String, f64)> = s
        .result
        .rows
        .iter()
        .filter_map(|r| Some((display(r.first()?), r.get(1)?.as_f64()?)))
        .collect();
    let (first, last) = (points.first()?, points.last()?);
    let max = points.iter().max_by(|a, b| a.1.total_cmp(&b.1))?;
    let min = points.iter().min_by(|a, b| a.1.total_cmp(&b.1))?;
    let total: f64 = points.iter().map(|p| p.1).sum();
    let share = |v: f64| {
        if total > 0.0 {
            format!(" ({:.0}% of the total)", v / total * 100.0)
        } else {
            String::new()
        }
    };
    let mut out = format!(
        "total {}; highest {} = {}{}; lowest {} = {}{}",
        fmt(total),
        max.0,
        fmt(max.1),
        share(max.1),
        min.0,
        fmt(min.1),
        share(min.1)
    );
    if s.chart == ChartKind::Line && first.1 != 0.0 {
        let change = (last.1 - first.1) / first.1.abs() * 100.0;
        let _ = write!(
            out,
            "; from {} ({}) to {} ({}): {change:+.0}%",
            first.0,
            fmt(first.1),
            last.0,
            fmt(last.1)
        );
    }
    Some(out)
}

/// Charts need a label column and a numeric one; anything else is shown as a table.
fn fit_chart(chart: ChartKind, r: &QueryResult) -> ChartKind {
    let chartable = r.columns.len() == 2
        && r.rows
            .iter()
            .all(|row| row.get(1).is_some_and(|v| v.is_number() || v.is_null()))
        && r.rows
            .iter()
            .any(|row| row.get(1).is_some_and(Value::is_number));
    match chart {
        ChartKind::Line if chartable && r.rows.len() >= 2 => ChartKind::Line,
        ChartKind::Bar | ChartKind::Line if chartable => ChartKind::Bar,
        _ => ChartKind::Table,
    }
}

#[derive(Deserialize)]
struct NarrativeReply {
    #[serde(default)]
    summary: String,
    #[serde(default)]
    highlights: Vec<String>,
    #[serde(default)]
    commentaries: Vec<String>,
}

fn narrative_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["summary", "highlights", "commentaries"],
        "properties": {
            "summary": { "type": "string" },
            "highlights": { "type": "array", "items": { "type": "string" } },
            "commentaries": { "type": "array", "items": { "type": "string" } },
        },
    })
}

/// Asks the AI to explain the results. Commentaries are attached to the sections in order.
pub async fn ai_narrative(
    model: &dyn Model,
    data: &str,
    brief: &Brief<'_>,
    fmt: &(dyn Fn(f64) -> String + Sync),
    sections: &mut [Section],
) -> Result<Narrative> {
    let mut prompt = format!(
        "{data}\n{}\nResults of the report's sections:\n",
        brief.head()
    );
    for (i, s) in sections.iter().enumerate() {
        let _ = write!(
            prompt,
            "\n## {}. {}\n{}",
            i + 1,
            s.title,
            table_text(&s.result, NARRATIVE_ROWS, Some(fmt))
        );
        if let Some(facts) = facts(s, fmt) {
            let _ = writeln!(prompt, "Facts (computed, exact): {facts}");
        }
    }
    let _ = write!(
        prompt,
        "\nWrite in {language}, for the reader described above (a business reader if none is):\n\
         - summary: two or three short paragraphs (separated by a blank line) with the main conclusions, citing concrete figures.\n\
         - highlights: three to five one-line key findings.\n\
         - commentaries: exactly {n} entries, one or two sentences on each section above, in the same order.\n\
         Use only figures that appear in the results or the facts, written exactly as shown (for example {example}). \
         Base every statement about highs, lows, trends and shares on the facts.\n",
        language = brief.language,
        n = sections.len(),
        example = fmt(1234567.89),
    );
    let reply: NarrativeReply =
        serde_json::from_value(model.json(SYSTEM, &prompt, &narrative_schema()).await?)
            .map_err(|e| Error::Provider(format!("The model's conclusions were malformed: {e}")))?;
    for (section, text) in sections.iter_mut().zip(reply.commentaries) {
        let text = text.trim();
        if !text.is_empty() {
            section.commentary = Some(text.to_owned());
        }
    }
    Ok(Narrative {
        summary: reply
            .summary
            .split("\n\n")
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect(),
        highlights: reply
            .highlights
            .into_iter()
            .map(|h| {
                h.trim()
                    .trim_start_matches(['-', '•', '*'])
                    .trim()
                    .to_owned()
            })
            .filter(|h| !h.is_empty())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::SourceColumn;

    fn col(name: &str, kind: ColumnKind, distinct: u64) -> ColumnProfile {
        ColumnProfile {
            name: name.into(),
            label: name.into(),
            kind,
            filled: 100,
            distinct,
            min: Value::Null,
            max: Value::Null,
            mean: None,
            sum: None,
            top: Vec::new(),
        }
    }

    const TITLES: Titles = Titles {
        by_month: "{measure} by month",
        count_by_month: "Records by month",
        by_dim: "{measure} by {dim}",
        count_by_dim: "Records by {dim}",
        top_rows: "Top rows by {measure}",
    };

    fn summary(profile: &[ColumnProfile]) -> SourceSummary {
        SourceSummary {
            file: "sales.csv".into(),
            table: "sales".into(),
            rows: 100,
            columns: profile
                .iter()
                .map(|c| SourceColumn {
                    name: c.name.clone(),
                    label: c.label.clone(),
                    kind: c.kind,
                })
                .collect(),
            format: crate::sources::SourceFormat::Csv {
                delimiter: ",".into(),
                encoding: "UTF-8".into(),
            },
        }
    }

    #[test]
    fn rules_pick_the_amount_the_trend_and_the_categories() {
        let profile = [
            col("order_id", ColumnKind::Integer, 100),
            col("date", ColumnKind::Date, 60),
            col("region", ColumnKind::Text, 4),
            col("customer", ColumnKind::Text, 100),
            col("units", ColumnKind::Integer, 9),
            col("total", ColumnKind::Decimal, 95),
        ];
        assert_eq!(primary_measure(&profile, 100).unwrap().name, "total");
        let plans = auto_plan(&summary(&profile), &profile, &TITLES);
        let titles: Vec<_> = plans.iter().map(|p| p.title.as_str()).collect();
        assert_eq!(titles, ["total by month", "total by region"]);
        assert_eq!(plans[0].chart, ChartKind::Line);
        assert!(
            plans[1].sql.contains("ROUND(SUM(\"total\"), 2)"),
            "{}",
            plans[1].sql
        );
    }

    #[test]
    fn without_numbers_the_rules_count_rows() {
        let profile = [col("status", ColumnKind::Text, 3)];
        let plans = auto_plan(&summary(&profile), &profile, &TITLES);
        assert_eq!(plans[0].title, "Records by status");
        assert!(plans[0].sql.contains("COUNT(*)"));
    }

    #[test]
    fn facts_spell_out_highs_lows_and_change() {
        let section = Section {
            title: "Sales by month".into(),
            chart: ChartKind::Line,
            result: QueryResult {
                columns: vec!["month".into(), "total".into()],
                rows: vec![
                    vec!["2026-01".into(), 200.into()],
                    vec!["2026-02".into(), 50.into()],
                    vec!["2026-03".into(), 250.into()],
                ],
                truncated: false,
            },
            commentary: None,
        };
        let facts = facts(&section, &|v| format!("{v:.0}")).unwrap();
        assert_eq!(
            facts,
            "total 500; highest 2026-03 = 250 (50% of the total); lowest 2026-02 = 50 (10% of the total); \
             from 2026-01 (200) to 2026-03 (250): +25%"
        );
    }

    #[test]
    fn charts_fall_back_to_tables_when_the_shape_is_wrong() {
        let result = |columns: &[&str], rows: Vec<Vec<Value>>| QueryResult {
            columns: columns.iter().map(|c| (*c).to_owned()).collect(),
            rows,
            truncated: false,
        };
        let two = result(
            &["a", "b"],
            vec![vec!["x".into(), 1.into()], vec!["y".into(), 2.into()]],
        );
        assert_eq!(fit_chart(ChartKind::Line, &two), ChartKind::Line);
        let one = result(&["a", "b"], vec![vec!["x".into(), 1.into()]]);
        assert_eq!(fit_chart(ChartKind::Line, &one), ChartKind::Bar);
        let text = result(&["a", "b"], vec![vec!["x".into(), "y".into()]]);
        assert_eq!(fit_chart(ChartKind::Bar, &text), ChartKind::Table);
        let wide = result(&["a", "b", "c"], vec![vec!["x".into(), 1.into(), 2.into()]]);
        assert_eq!(fit_chart(ChartKind::Bar, &wide), ChartKind::Table);
    }
}
