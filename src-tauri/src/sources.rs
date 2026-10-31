//! Report data sources. A CSV file or a spreadsheet sheet is imported once into
//! a cached SQLite file and from then on read through the same read-only
//! `Database` as any other database, so queries written by the AI get the same
//! guarantees.
//!
//! The import copes with what real exports look like: `,` `;` tab or `|`
//! delimiters, UTF-8/UTF-16 with or without BOM or Windows-1252, decimal
//! commas, thousands separators, currency signs, day-first dates and title
//! lines above the header. Spreadsheet cells go through the same type rules.

use std::borrow::Cow;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};

use calamine::Reader;
use chrono::{NaiveDate, NaiveTime};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{AssertSqlSafe, Connection, SqliteConnection};

use crate::database::{Database, Driver, QueryResult};
use crate::error::{Error, Result};

const MAX_FILE_BYTES: u64 = 100 * 1024 * 1024;
/// Bump whenever the import rules change so cached imports are rebuilt.
const IMPORT_VERSION: u32 = 2;
const DELIMITERS: [u8; 4] = *b",;\t|";
/// Extensions read as workbooks; anything else is read as delimited text.
pub const SPREADSHEETS: [&str; 5] = ["xlsx", "xlsm", "xlsb", "xls", "ods"];
const NULLS: [&str; 5] = ["null", "na", "n/a", "nan", "#n/a"];
/// Identifiers a model would be tempted to write unquoted; they get a trailing `_`.
const RESERVED: [&str; 14] = [
    "order", "group", "select", "from", "where", "table", "limit", "index", "by", "case", "when",
    "end", "values", "default",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum ColumnKind {
    Integer,
    Decimal,
    Date,
    Text,
}

impl ColumnKind {
    pub fn is_number(self) -> bool {
        matches!(self, Self::Integer | Self::Decimal)
    }

    fn sql_type(self) -> &'static str {
        match self {
            Self::Integer => "INTEGER",
            Self::Decimal => "REAL",
            // Dates are stored as ISO text, which SQLite's date functions understand.
            Self::Date | Self::Text => "TEXT",
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceColumn {
    /// SQL identifier: lowercase ASCII snake_case, safe to write unquoted.
    pub name: String,
    /// The header as written in the file.
    pub label: String,
    pub kind: ColumnKind,
}

#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SourceSummary {
    pub file: String,
    pub table: String,
    pub rows: u32,
    pub columns: Vec<SourceColumn>,
    pub format: SourceFormat,
}

impl SourceSummary {
    /// The file name, plus the sheet when the workbook has several.
    pub fn name(&self) -> String {
        match &self.format {
            SourceFormat::Spreadsheet { sheet, sheets } if sheets.len() > 1 => {
                format!("{} · {sheet}", self.file)
            }
            _ => self.file.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SourceFormat {
    /// `delimiter` is the character itself, or `tab`.
    Csv { delimiter: String, encoding: String },
    /// The imported sheet and every visible worksheet, in workbook order.
    Spreadsheet { sheet: String, sheets: Vec<String> },
}

/// An imported file, ready to be queried.
pub struct FileSource {
    pub summary: SourceSummary,
    db_path: PathBuf,
}

impl FileSource {
    /// Imports `file` (for a workbook, `sheet` or else its first sheet with data),
    /// reusing a previous import while the file is unchanged.
    pub async fn import(file: &Path, sheet: Option<&str>, cache_dir: &Path) -> Result<Self> {
        let meta =
            std::fs::metadata(file).map_err(|e| Error::Io(format!("{}: {e}", file.display())))?;
        if meta.len() > MAX_FILE_BYTES {
            return Err(Error::Io(format!(
                "{} is larger than {} MB",
                file.display(),
                MAX_FILE_BYTES / 1024 / 1024
            )));
        }
        let mut hasher = DefaultHasher::new();
        (
            IMPORT_VERSION,
            file,
            meta.len(),
            meta.modified().ok(),
            sheet,
        )
            .hash(&mut hasher);
        let dir = cache_dir.join("sources");
        let db_path = dir.join(format!("{:016x}.db", hasher.finish()));
        let summary_path = db_path.with_extension("json");

        if let Ok(json) = std::fs::read(&summary_path)
            && let Ok(summary) = serde_json::from_slice(&json)
            && db_path.is_file()
        {
            return Ok(Self { summary, db_path });
        }

        let file_name = file
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let stem = file
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        let table = identifier(&stem, "data");
        let path = file.to_owned();
        let sheet = sheet.map(str::to_owned);
        let parsed = tokio::task::spawn_blocking(move || {
            if is_spreadsheet(&path) {
                parse_workbook(&path, sheet.as_deref())
            } else {
                parse_csv(&std::fs::read(&path)?)
            }
        })
        .await
        .map_err(|e| Error::Io(e.to_string()))??;

        std::fs::create_dir_all(&dir)?;
        let partial = db_path.with_extension("partial");
        let _ = std::fs::remove_file(&partial);
        write_table(&partial, &table, &parsed).await?;
        std::fs::rename(&partial, &db_path)?;

        let summary = SourceSummary {
            file: file_name,
            table,
            rows: u32::try_from(parsed.rows.len()).unwrap_or(u32::MAX),
            columns: parsed.columns,
            format: parsed.format,
        };
        std::fs::write(
            &summary_path,
            serde_json::to_vec(&summary).unwrap_or_default(),
        )?;
        Ok(Self { summary, db_path })
    }

    pub async fn open(&self) -> Result<Database> {
        Database::open(Driver::Sqlite, &self.db_path.to_string_lossy()).await
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
}

struct Parsed {
    format: SourceFormat,
    columns: Vec<SourceColumn>,
    rows: Vec<Vec<Cell>>,
}

fn is_spreadsheet(file: &Path) -> bool {
    file.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| SPREADSHEETS.iter().any(|s| e.eq_ignore_ascii_case(s)))
}

fn parse_csv(bytes: &[u8]) -> Result<Parsed> {
    let (text, encoding) = decode(bytes);
    let delimiter = sniff_delimiter(&text);
    let records: Vec<Vec<String>> = csv::ReaderBuilder::new()
        .delimiter(delimiter)
        .has_headers(false)
        .flexible(true)
        .trim(csv::Trim::All)
        .from_reader(text.as_bytes())
        .records()
        .map(|r| r.map(|r| r.iter().map(str::to_owned).collect()))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| Error::Io(format!("could not read the CSV file: {e}")))?;
    // Semicolon files come from locales that write decimal commas; prefer that reading when both fit.
    let decimal = if delimiter == b';' {
        Decimal::Comma
    } else {
        Decimal::Point
    };
    let (columns, rows) = tabulate(records, decimal)?;
    Ok(Parsed {
        format: SourceFormat::Csv {
            delimiter: match delimiter {
                b'\t' => "tab".into(),
                d => char::from(d).to_string(),
            },
            encoding: encoding.into(),
        },
        columns,
        rows,
    })
}

/// One sheet of a workbook: `sheet`, or else the first visible worksheet with
/// rows under its header, so cover and notes sheets are skipped.
fn parse_workbook(file: &Path, sheet: Option<&str>) -> Result<Parsed> {
    let failed = |e: calamine::Error| Error::Io(format!("could not read the spreadsheet: {e}"));
    let mut book = calamine::open_workbook_auto(file).map_err(failed)?;
    let sheets: Vec<String> = book
        .sheets_metadata()
        .iter()
        .filter(|s| {
            s.typ == calamine::SheetType::WorkSheet && s.visible == calamine::SheetVisible::Visible
        })
        .map(|s| s.name.clone())
        .collect();
    let mut read = |name: &str| {
        let range = book.worksheet_range(name).map_err(failed)?;
        let records = range
            .rows()
            .map(|r| r.iter().map(cell_text).collect())
            .collect();
        // Numbers are stored as numbers, so their text always uses a decimal point.
        tabulate(records, Decimal::Point)
    };
    let (sheet, (columns, rows)) = match sheet {
        Some(name) => (name.to_owned(), read(name)?),
        None => sheets
            .iter()
            .find_map(|name| {
                read(name)
                    .ok()
                    .filter(|(_, rows)| !rows.is_empty())
                    .map(|table| (name.clone(), table))
            })
            .ok_or_else(|| Error::Io("no sheet in this workbook has data".into()))?,
    };
    Ok(Parsed {
        format: SourceFormat::Spreadsheet { sheet, sheets },
        columns,
        rows,
    })
}

/// A spreadsheet cell written the way a CSV export would write it.
fn cell_text(cell: &calamine::Data) -> String {
    use calamine::Data;
    match cell {
        Data::Int(v) => v.to_string(),
        Data::Float(v) => v.to_string(),
        Data::Bool(v) => v.to_string(),
        Data::String(v) | Data::DateTimeIso(v) | Data::DurationIso(v) => v.trim().to_owned(),
        Data::DateTime(d) if d.is_duration() => d
            .as_duration()
            .map(|d| {
                let s = d.num_seconds();
                format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
            })
            .unwrap_or_default(),
        Data::DateTime(d) => d
            .as_datetime()
            .map(|d| {
                let format = if d.time() == NaiveTime::MIN {
                    "%Y-%m-%d"
                } else {
                    "%Y-%m-%d %H:%M:%S"
                };
                d.format(format).to_string()
            })
            .unwrap_or_default(),
        Data::Error(_) | Data::Empty => String::new(),
    }
}

/// Header, column names and typed rows from raw text records.
fn tabulate(
    mut records: Vec<Vec<String>>,
    decimal: Decimal,
) -> Result<(Vec<SourceColumn>, Vec<Vec<Cell>>)> {
    records.retain(|r| r.iter().any(|v| !v.is_empty()));
    // Titles and notes above the header fill a cell or two; the header fills most of its row.
    let filled = |r: &Vec<String>| r.iter().filter(|v| !v.is_empty()).count();
    let widest = records.iter().map(filled).max().unwrap_or(0);
    let start = records
        .iter()
        .position(|r| filled(r) * 2 > widest)
        .unwrap_or(0);
    records.drain(..start);
    if records.is_empty() {
        return Err(Error::Io("there is no data in this file".into()));
    }
    let header = records.remove(0);
    let width = records.iter().map(Vec::len).fold(header.len(), usize::max);

    let mut names: Vec<String> = Vec::with_capacity(width);
    let mut columns = Vec::with_capacity(width);
    let mut converters = Vec::with_capacity(width);
    for i in 0..width {
        let label = header.get(i).cloned().unwrap_or_default();
        let mut name = identifier(&label, &format!("column_{}", i + 1));
        if names.contains(&name) {
            let base = name.clone();
            name = (2..)
                .map(|n| format!("{base}_{n}"))
                .find(|n| !names.contains(n))
                .unwrap_or(base);
        }
        let values: Vec<&str> = records
            .iter()
            .filter_map(|r| r.get(i).map(String::as_str))
            .filter(|v| !is_null(v))
            .collect();
        let converter = infer(&values, decimal);
        names.push(name.clone());
        columns.push(SourceColumn {
            label: if label.is_empty() {
                name.clone()
            } else {
                label
            },
            name,
            kind: converter.kind(),
        });
        converters.push(converter);
    }

    let rows = records
        .iter()
        .map(|r| {
            converters
                .iter()
                .enumerate()
                .map(|(i, conv)| match r.get(i) {
                    Some(v) if !is_null(v) => conv.convert(v),
                    _ => Cell::Null,
                })
                .collect()
        })
        .collect();
    Ok((columns, rows))
}

/// UTF-8 or UTF-16 when a BOM says so or the bytes are valid UTF-8; otherwise
/// Windows-1252, which is what Excel writes on most Western-language systems.
pub(crate) fn decode(bytes: &[u8]) -> (Cow<'_, str>, &'static str) {
    if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        return (enc.decode_without_bom_handling(&bytes[bom..]).0, enc.name());
    }
    match std::str::from_utf8(bytes) {
        Ok(text) => (Cow::Borrowed(text), "UTF-8"),
        Err(_) => (
            encoding_rs::WINDOWS_1252
                .decode_without_bom_handling(bytes)
                .0,
            "windows-1252",
        ),
    }
}

/// The delimiter that splits the first lines into the most columns, consistently.
fn sniff_delimiter(text: &str) -> u8 {
    let sample: String = text.lines().take(50).collect::<Vec<_>>().join("\n");
    let score = |d: u8| {
        let counts: Vec<usize> = csv::ReaderBuilder::new()
            .delimiter(d)
            .has_headers(false)
            .flexible(true)
            .from_reader(sample.as_bytes())
            .records()
            .filter_map(|r| r.ok().map(|r| r.len()))
            .collect();
        let Some(&first) = counts.first() else {
            return (0, 0);
        };
        let consistent = counts.iter().filter(|&&c| c == first).count() * 100 / counts.len();
        if first < 2 {
            (0, 0)
        } else {
            (consistent, first)
        }
    };
    // Reversed so that ties go to the most common delimiter.
    DELIMITERS
        .into_iter()
        .rev()
        .max_by_key(|&d| score(d))
        .filter(|&d| score(d).1 > 1)
        .unwrap_or(b',')
}

fn is_null(v: &str) -> bool {
    v.is_empty() || NULLS.iter().any(|n| v.eq_ignore_ascii_case(n))
}

/// Lowercase ASCII snake_case, so a model can write it in SQL without quoting.
pub(crate) fn identifier(label: &str, fallback: &str) -> String {
    let mut out = String::with_capacity(label.len());
    for c in label.chars().flat_map(char::to_lowercase).map(fold_accent) {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.is_empty() && !out.ends_with('_') {
            out.push('_');
        }
    }
    let mut out = out.trim_end_matches('_').to_owned();
    if out.is_empty() {
        out = fallback.to_owned();
    }
    if out.starts_with(|c: char| c.is_ascii_digit()) {
        out.insert_str(0, "c_");
    }
    if RESERVED.contains(&out.as_str()) {
        out.push('_');
    }
    out
}

fn fold_accent(c: char) -> char {
    match c {
        'á' | 'à' | 'â' | 'ä' | 'ã' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'ö' | 'õ' | 'ø' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ñ' => 'n',
        'ç' => 'c',
        'ý' | 'ÿ' => 'y',
        other => other,
    }
}

/// Which character separates decimals in a column's numbers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decimal {
    Point,
    Comma,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DateOrder {
    Ymd,
    Dmy,
    Mdy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Converter {
    Integer(Decimal),
    Real(Decimal),
    Date { order: DateOrder, with_time: bool },
    Text,
}

impl Converter {
    fn kind(self) -> ColumnKind {
        match self {
            Self::Integer(_) => ColumnKind::Integer,
            Self::Real(_) => ColumnKind::Decimal,
            Self::Date { .. } => ColumnKind::Date,
            Self::Text => ColumnKind::Text,
        }
    }

    fn convert(self, v: &str) -> Cell {
        let converted = match self {
            Self::Integer(d) => parse_number(v, d).and_then(|n| n.int).map(Cell::Int),
            Self::Real(d) => parse_number(v, d).map(|n| Cell::Real(n.value)),
            Self::Date { order, with_time } => parse_date(v, order).map(|d| {
                Cell::Text(if with_time {
                    d.format("%Y-%m-%d %H:%M:%S").to_string()
                } else {
                    d.format("%Y-%m-%d").to_string()
                })
            }),
            Self::Text => None,
        };
        converted.unwrap_or_else(|| Cell::Text(v.to_owned()))
    }
}

/// The most specific type every value in the column fits, trying `decimal` first.
fn infer(values: &[&str], decimal: Decimal) -> Converter {
    if values.is_empty() {
        return Converter::Text;
    }
    let styles = match decimal {
        Decimal::Comma => [Decimal::Comma, Decimal::Point],
        Decimal::Point => [Decimal::Point, Decimal::Comma],
    };
    for style in styles {
        let numbers: Option<Vec<Number>> = values.iter().map(|v| parse_number(v, style)).collect();
        if let Some(numbers) = numbers {
            return if numbers.iter().all(|n| n.int.is_some()) {
                Converter::Integer(style)
            } else {
                Converter::Real(style)
            };
        }
    }
    if let Some(order) = date_order(values) {
        return Converter::Date {
            order,
            with_time: values.iter().any(|v| v.contains(':')),
        };
    }
    Converter::Text
}

struct Number {
    value: f64,
    int: Option<i64>,
}

fn parse_number(raw: &str, style: Decimal) -> Option<Number> {
    let (dec, group) = match style {
        Decimal::Point => ('.', ','),
        Decimal::Comma => (',', '.'),
    };
    let mut s = raw.trim();
    let mut negative = false;
    if let Some(inner) = s.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        negative = true;
        s = inner.trim();
    }
    let s = s.trim_end_matches('%').trim();
    let s = s
        .trim_matches(|c| matches!(c, '$' | '€' | '£' | '¥'))
        .trim();
    let (sign, s) = match s.strip_prefix('-').or_else(|| s.strip_prefix('−')) {
        Some(rest) => (true, rest.trim_start_matches(['$', '€', '£', '¥']).trim()),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let negative = negative ^ sign;

    let (int_part, frac) = match s.split_once(dec) {
        Some((i, f)) => (i, Some(f)),
        None => (s, None),
    };
    let groups: Vec<&str> = int_part.split([group, ' ', '\u{a0}', '\u{202f}']).collect();
    let valid_groups = groups.len() == 1
        || (matches!(groups[0].len(), 1..=3) && groups[1..].iter().all(|g| g.len() == 3));
    let digits: String = groups.concat();
    if digits.is_empty() || !valid_groups || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if let Some(f) = frac
        && (f.is_empty() || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    // Leading zeros mean a code (postal code, SKU), not a quantity.
    if digits.len() > 1 && digits.starts_with('0') && frac.is_none() {
        return None;
    }
    let text = match frac {
        Some(f) => format!("{}{digits}.{f}", if negative { "-" } else { "" }),
        None => format!("{}{digits}", if negative { "-" } else { "" }),
    };
    let value: f64 = text.parse().ok()?;
    Some(Number {
        value,
        int: frac.is_none().then(|| text.parse().ok()).flatten(),
    })
}

/// Splits `2026-01-31`, `31/01/2026 14:05` and the like into numeric parts.
/// The three numbers of a date in written order, plus hours, minutes and seconds.
type DateParts = ([u32; 3], Option<(u32, u32, u32)>);

fn date_parts(v: &str) -> Option<DateParts> {
    let v = v.trim();
    let (date, time) = match v.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t.trim())),
        None => (v, None),
    };
    let parts: Vec<&str> = date.split(['-', '/', '.']).collect();
    let [a, b, c] = parts.as_slice() else {
        return None;
    };
    let num = |s: &str| -> Option<u32> {
        (!s.is_empty() && s.len() <= 4 && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse().ok())
            .flatten()
    };
    let parts = [num(a)?, num(b)?, num(c)?];
    // Either the year comes first (4 digits) or last (2 or 4 digits).
    let year_first = a.len() == 4 && b.len() <= 2 && c.len() <= 2;
    let year_last = a.len() <= 2 && b.len() <= 2 && matches!(c.len(), 2 | 4);
    if !(year_first || year_last) {
        return None;
    }
    let time = match time {
        None => None,
        Some(t) => {
            let t = t.trim_end_matches('Z');
            let t = t.split(['+', '.']).next().unwrap_or(t);
            let mut hms = t.split(':').map(|p| p.parse::<u32>().ok());
            Some((
                hms.next()??,
                hms.next().flatten().unwrap_or(0),
                hms.next().flatten().unwrap_or(0),
            ))
        }
    };
    Some((parts, time))
}

/// The one reading under which every value is a real calendar date, if any.
fn date_order(values: &[&str]) -> Option<DateOrder> {
    let parsed: Vec<_> = values
        .iter()
        .map(|v| date_parts(v))
        .collect::<Option<_>>()?;
    let year_first = values.iter().all(|v| {
        v.trim()
            .split(['-', '/', '.'])
            .next()
            .is_some_and(|p| p.len() == 4)
    });
    let candidates: &[DateOrder] = if year_first {
        &[DateOrder::Ymd]
    } else if parsed.iter().any(|(p, _)| p[1] > 12) {
        &[DateOrder::Mdy]
    } else {
        &[DateOrder::Dmy, DateOrder::Mdy]
    };
    candidates
        .iter()
        .copied()
        .find(|&order| values.iter().all(|v| parse_date(v, order).is_some()))
}

fn parse_date(v: &str, order: DateOrder) -> Option<chrono::NaiveDateTime> {
    let ([a, b, c], time) = date_parts(v)?;
    let (y, m, d) = match order {
        DateOrder::Ymd => (a, b, c),
        DateOrder::Dmy => (c, b, a),
        DateOrder::Mdy => (c, a, b),
    };
    let y = if y < 100 { 2000 + y } else { y };
    let (h, min, s) = time.unwrap_or((0, 0, 0));
    NaiveDate::from_ymd_opt(i32::try_from(y).ok()?, m, d)?.and_hms_opt(h, min, s)
}

async fn write_table(path: &Path, table: &str, parsed: &Parsed) -> Result<()> {
    let opts = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Delete)
        .synchronous(SqliteSynchronous::Off);
    let mut conn = SqliteConnection::connect_with(&opts).await?;
    let cols: Vec<String> = parsed
        .columns
        .iter()
        .map(|c| format!("\"{}\" {}", c.name, c.kind.sql_type()))
        .collect();
    let placeholders = vec!["?"; parsed.columns.len()].join(", ");
    // Identifiers come from `identifier()`, which only emits [a-z0-9_].
    let create = format!("CREATE TABLE \"{table}\" ({})", cols.join(", "));
    let insert = format!("INSERT INTO \"{table}\" VALUES ({placeholders})");

    let mut tx = conn.begin().await?;
    sqlx::query(AssertSqlSafe(create)).execute(&mut *tx).await?;
    for row in &parsed.rows {
        let mut q = sqlx::query(AssertSqlSafe(insert.as_str()));
        for cell in row {
            q = match cell {
                Cell::Null => q.bind(None::<String>),
                Cell::Int(v) => q.bind(*v),
                Cell::Real(v) => q.bind(*v),
                Cell::Text(v) => q.bind(v.as_str()),
            };
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    conn.close().await?;
    Ok(())
}

/// What a report needs to know about one column, computed with read-only queries.
#[derive(Debug, Clone, Serialize)]
pub struct ColumnProfile {
    pub name: String,
    pub label: String,
    pub kind: ColumnKind,
    pub filled: u64,
    pub distinct: u64,
    pub min: Value,
    pub max: Value,
    pub mean: Option<f64>,
    pub sum: Option<f64>,
    /// Most frequent values with their counts (text columns).
    pub top: Vec<(String, u64)>,
}

pub async fn profile(db: &Database, summary: &SourceSummary) -> Result<Vec<ColumnProfile>> {
    let t = &summary.table;
    let mut out = Vec::with_capacity(summary.columns.len());
    for c in &summary.columns {
        let n = &c.name;
        let numeric = if c.kind.is_number() {
            format!(", AVG(\"{n}\"), SUM(\"{n}\")")
        } else {
            String::new()
        };
        let r = db
            .query(&format!(
                "SELECT COUNT(\"{n}\"), COUNT(DISTINCT \"{n}\"), MIN(\"{n}\"), MAX(\"{n}\"){numeric} FROM \"{t}\""
            ))
            .await?;
        let row = r.rows.into_iter().next().unwrap_or_default();
        let cell = |i: usize| row.get(i).cloned().unwrap_or(Value::Null);
        let top = if c.kind == ColumnKind::Text {
            db.query(&format!(
                "SELECT \"{n}\", COUNT(*) FROM \"{t}\" WHERE \"{n}\" IS NOT NULL GROUP BY 1 ORDER BY 2 DESC, 1 LIMIT 5"
            ))
            .await?
            .rows
            .into_iter()
            .filter_map(|r| Some((display(r.first()?), r.get(1)?.as_u64()?)))
            .collect()
        } else {
            Vec::new()
        };
        out.push(ColumnProfile {
            name: c.name.clone(),
            label: c.label.clone(),
            kind: c.kind,
            filled: cell(0).as_u64().unwrap_or(0),
            distinct: cell(1).as_u64().unwrap_or(0),
            min: cell(2),
            max: cell(3),
            mean: cell(4).as_f64(),
            sum: cell(5).as_f64(),
            top,
        });
    }
    Ok(out)
}

/// First rows of the table, for previews and as examples for the AI.
pub async fn sample(db: &Database, summary: &SourceSummary, rows: usize) -> Result<QueryResult> {
    db.query(&format!("SELECT * FROM \"{}\" LIMIT {rows}", summary.table))
        .await
}

/// A cell as plain text, for labels and prompts.
pub fn display(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn kinds(p: &Parsed) -> Vec<ColumnKind> {
        p.columns.iter().map(|c| c.kind).collect()
    }

    fn csv(delimiter: &str, encoding: &str) -> SourceFormat {
        SourceFormat::Csv {
            delimiter: delimiter.into(),
            encoding: encoding.into(),
        }
    }

    /// A workbook shaped like a real export: a notes sheet first, a title above
    /// the header, real dates, and codes stored as text.
    pub(crate) fn sales_workbook(path: &Path) {
        use rust_xlsxwriter::{ExcelDateTime, Format, Workbook};
        let mut book = Workbook::new();
        let date = Format::new().set_num_format("dd/mm/yyyy");
        book.add_worksheet()
            .set_name("Notas")
            .unwrap()
            .write_string(0, 0, "Exportado del sistema de ventas")
            .unwrap();
        let sales = book.add_worksheet().set_name("Ventas").unwrap();
        sales.write_string(0, 0, "Reporte de ventas 2026").unwrap();
        for (col, header) in ["Fecha", "Región", "Código", "Unidades", "Total"]
            .into_iter()
            .enumerate()
        {
            sales.write_string(2, col as u16, header).unwrap();
        }
        let rows = [
            ((2026, 1, 5), "Norte", "007", 3.0, 1299.5),
            ((2026, 1, 6), "Sur", "010", 1.0, 80.0),
            ((2026, 2, 7), "Norte", "007", 2.0, 450.25),
        ];
        for (i, ((y, m, d), region, code, units, total)) in rows.into_iter().enumerate() {
            let row = 3 + i as u32;
            let day = ExcelDateTime::from_ymd(y, m, d).unwrap();
            sales
                .write_datetime_with_format(row, 0, day, &date)
                .unwrap();
            sales.write_string(row, 1, region).unwrap();
            sales.write_string(row, 2, code).unwrap();
            sales.write_number(row, 3, units).unwrap();
            sales.write_number(row, 4, total).unwrap();
        }
        let goals = book.add_worksheet().set_name("Metas").unwrap();
        goals.write_string(0, 0, "Región").unwrap();
        goals.write_string(0, 1, "Meta").unwrap();
        for (row, (region, goal)) in [(1, ("Norte", 1000.0)), (2, ("Sur", 800.0))] {
            goals.write_string(row, 0, region).unwrap();
            goals.write_number(row, 1, goal).unwrap();
        }
        book.save(path).unwrap();
    }

    #[test]
    fn skips_title_lines_above_the_header() {
        let p = parse_csv(
            b"Reporte de ventas\nGenerado el 01/02/2026\n\nregion,total\nNorte,10\nSur,5\n",
        )
        .unwrap();
        let names: Vec<_> = p.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["region", "total"]);
        assert_eq!(p.rows.len(), 2);
    }

    #[tokio::test]
    async fn imports_the_first_sheet_with_data_or_the_chosen_one() {
        let dir = std::env::temp_dir().join(format!("mcforge-xlsx-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Ventas 2026.xlsx");
        sales_workbook(&file);

        let source = FileSource::import(&file, None, &dir).await.unwrap();
        let s = &source.summary;
        assert_eq!(
            s.format,
            SourceFormat::Spreadsheet {
                sheet: "Ventas".into(),
                sheets: vec!["Notas".into(), "Ventas".into(), "Metas".into()],
            }
        );
        assert_eq!(s.name(), "Ventas 2026.xlsx · Ventas");
        let labels: Vec<_> = s.columns.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["Fecha", "Región", "Código", "Unidades", "Total"]);
        use ColumnKind::*;
        let kinds: Vec<_> = s.columns.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, [Date, Text, Text, Integer, Decimal]);
        let db = source.open().await.unwrap();
        let r = db
            .query("SELECT fecha, codigo, total FROM ventas_2026 ORDER BY fecha")
            .await
            .unwrap();
        assert_eq!(
            r.rows[0],
            [
                Value::from("2026-01-05"),
                Value::from("007"),
                Value::from(1299.5)
            ]
        );

        let goals = FileSource::import(&file, Some("Metas"), &dir)
            .await
            .unwrap();
        assert_eq!(goals.summary.rows, 2);
        assert_eq!(goals.summary.columns[1].kind, Integer);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn reads_a_plain_comma_file() {
        let p = parse_csv(b"Order ID,Date,Region,Units,Revenue\n1,2026-01-05,North,3,1299.50\n2,2026-01-06,South,1,80\n").unwrap();
        assert_eq!(p.format, csv(",", "UTF-8"));
        let names: Vec<_> = p.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["order_id", "date", "region", "units", "revenue"]);
        use ColumnKind::*;
        assert_eq!(kinds(&p), [Integer, Date, Text, Integer, Decimal]);
        assert_eq!(p.rows[0][4], Cell::Real(1299.5));
        assert_eq!(p.rows[1][1], Cell::Text("2026-01-06".into()));
    }

    #[test]
    fn reads_a_spanish_excel_export() {
        // Windows-1252, semicolons, decimal commas, thousands dots and day-first dates.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"Fecha;Regi\xf3n;Importe;C\xf3digo\n");
        bytes.extend_from_slice(b"31/01/2026;Norte;1.234,56;007\n");
        bytes.extend_from_slice(b"02/02/2026;Sur;12,5;010\n");
        let p = parse_csv(&bytes).unwrap();
        assert_eq!(p.format, csv(";", "windows-1252"));
        let names: Vec<_> = p.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["fecha", "region", "importe", "codigo"]);
        assert_eq!(p.columns[1].label, "Región");
        use ColumnKind::*;
        assert_eq!(kinds(&p), [Date, Text, Decimal, Text]);
        assert_eq!(p.rows[0][0], Cell::Text("2026-01-31".into()));
        assert_eq!(p.rows[0][2], Cell::Real(1234.56));
        assert_eq!(p.rows[1][2], Cell::Real(12.5));
        assert_eq!(p.rows[0][3], Cell::Text("007".into()));
    }

    #[test]
    fn handles_bom_tabs_blanks_and_odd_headers() {
        let p = parse_csv(
            "\u{feff}Name\t\tName\torder\n\"Ana\"\t$1,200\tx\t\n\t\t\t\nLuis\t-$30\ty\tNA\n"
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(p.format, csv("tab", "UTF-8"));
        let names: Vec<_> = p.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["name", "column_2", "name_2", "order_"]);
        assert_eq!(p.rows.len(), 2, "blank lines are skipped");
        assert_eq!(p.rows[0][1], Cell::Int(1200));
        assert_eq!(p.rows[1][1], Cell::Int(-30));
        assert_eq!(p.rows[1][3], Cell::Null);
    }

    #[test]
    fn month_first_dates_are_detected_from_the_data() {
        let p = parse_csv(b"when,v\n01/31/2026 08:30,1\n02/01/2026,2\n").unwrap();
        assert_eq!(p.columns[0].kind, ColumnKind::Date);
        assert_eq!(p.rows[0][0], Cell::Text("2026-01-31 08:30:00".into()));
        assert_eq!(p.rows[1][0], Cell::Text("2026-02-01 00:00:00".into()));
    }

    #[test]
    fn mixed_values_stay_text() {
        let p = parse_csv(b"a,b\n1,2026-01-01\nx,soon\n").unwrap();
        assert_eq!(kinds(&p), [ColumnKind::Text, ColumnKind::Text]);
    }

    #[test]
    fn identifiers_are_safe_sql() {
        assert_eq!(identifier("Año de venta (MXN)", "x"), "ano_de_venta_mxn");
        assert_eq!(identifier("2026 total", "x"), "c_2026_total");
        assert_eq!(identifier("  ", "fallback"), "fallback");
        assert_eq!(identifier("Group", "x"), "group_");
    }

    #[tokio::test]
    async fn imports_once_and_profiles_through_the_read_only_api() {
        let dir = std::env::temp_dir().join(format!("mcforge-csv-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Ventas 2026.csv");
        std::fs::write(
            &file,
            "fecha;region;total\n05/01/2026;Norte;10,5\n06/01/2026;Sur;20\n07/02/2026;Norte;5\n",
        )
        .unwrap();

        let source = FileSource::import(&file, None, &dir).await.unwrap();
        assert_eq!(source.summary.table, "ventas_2026");
        assert_eq!(source.summary.rows, 3);
        let db = source.open().await.unwrap();
        assert!(
            db.query("DELETE FROM ventas_2026").await.is_err(),
            "imports are read-only"
        );

        let profile = profile(&db, &source.summary).await.unwrap();
        assert_eq!(profile[0].min, Value::from("2026-01-05"));
        assert_eq!(profile[1].top, [("Norte".into(), 2), ("Sur".into(), 1)]);
        assert_eq!(profile[2].sum, Some(35.5));

        let again = FileSource::import(&file, None, &dir).await.unwrap();
        assert_eq!(
            again.db_path, source.db_path,
            "unchanged files reuse the import"
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
