# Report templates (planned for F3)

Reports are MCForge's core feature. The AI never writes the document itself:

1. The model gathers data from any connection (database, files, web…) and returns it as JSON matching the template's fields.
2. The report engine renders the template deterministically, so layout, fonts and numbers come out the same every time.

| Format | Template | Engine |
|---|---|---|
| PDF | `.typ` ([Typst](https://typst.app)) | Typst, embedded |
| Excel | `.xlsx` with `{{placeholders}}` in cells | umya-spreadsheet |
| Word | `.docx` with `{{placeholders}}` | XML fill via minijinja |
| HTML / Markdown | `.html`, `.md` | minijinja |

Each template ships with a small manifest describing its fields, so the model knows exactly what to collect:

```toml
name = "Monthly sales"
format = "pdf"
file = "monthly-sales.typ"

[[fields]]
key = "month"
type = "string"

[[fields]]
key = "rows"
type = "table"
columns = ["region", "units", "revenue"]
```

Example templates will live in `templates/reports/`.
