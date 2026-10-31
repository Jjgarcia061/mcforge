# Report templates

Reports are MCForge's core feature. The AI never writes the document itself, and never computes a figure:

1. The data source is opened read-only. A CSV file is imported once into a cached SQLite table (delimiter, encoding, decimal commas and day-first dates are detected) and queried like any database.
2. **Plan.** The model sees the columns, their ranges and a few rows, plus the user's question, and answers with sections: a title, one `SELECT` query and a chart type (`bar`, `line` or `table`). Queries run through the read-only database; failed ones go back to the model once to be fixed. Without an AI engine, simple rules pick a monthly trend and the main breakdowns instead.
3. **Explain.** The model gets every result plus computed facts (highs, lows, shares, change) and writes the summary, key findings and one comment per section. It is told to use only those figures, written in the user's number format.
4. **Render.** The template is filled deterministically: same data, same document.

Every model answer is requested as JSON matching a schema (Ollama `format`, OpenAI structured outputs, a forced tool call for Anthropic), and a failing step never blocks the report: it falls back to the rules and tells the user why.

## Template layout

A template is a folder in `templates/reports/`:

```
templates/reports/data-overview/
├── template.toml   # names, descriptions and labels per language
└── report.html     # MiniJinja template, HTML-escaped
```

```toml
file = "report.html"

[locales.en]
name = "Data overview"
description = "Key figures, trends and breakdowns for any table."

[locales.en.labels]
summary = "Summary"
# …

[locales.es]
name = "Resumen de datos"
# …
```

An `en` locale is required and every locale must define the same labels (CI checks both). The template receives `title`, `question`, `engine`, `file`, `generated_at`, `summary` (paragraphs), `highlights`, `figures`, `sections` (each with `title`, `commentary`, an inline SVG `chart` or a `table`), `columns`, `preview` and the labels as `t`. Charts are built by the engine with every label escaped; they are the only value a template may print with `| safe`.

## Coming next

| Format | Engine |
|---|---|
| PDF | Typst, embedded (today: the HTML report prints cleanly to PDF) |
| Excel | `.xlsx` with `{{placeholders}}` in cells |
| Word | `.docx` with `{{placeholders}}` |

Databases configured in the setup will be report sources too, through the same read-only `Database` type.
