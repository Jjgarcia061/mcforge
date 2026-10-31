# Contributing to MCForge

Thanks for helping! Contributions of every size are welcome.

## Ways to help

- **Add a connector**: one TOML file in `catalog/connectors/`. See [docs/adding-a-connector.md](docs/adding-a-connector.md).
- **Add or tune a model** in `catalog/models.toml` (sizes, memory requirements, tool-calling quality).
- **Add a report template** in `templates/reports/`. See [docs/report-templates.md](docs/report-templates.md).
- **Translate** the UI: `src/i18n/<lang>.json`.
- **Fix bugs / build features**: look for issues labelled `good first issue`.

## Ground rules

- One concern per pull request; keep diffs focused.
- No duplicated logic: if two places need the same thing, it lives in one module.
- Every behaviour change ships with a test.
- Never commit secrets. Use the git-ignored `lab/` folder for local experiments and `.env` files.
- Third-party MCP servers must be actively maintained and must declare every tool with its `risk` level.

## Testing databases

SQLite tests always run. PostgreSQL and MySQL tests run against local servers started with Docker, preloaded with the demo data in `src-tauri/tests/databases/sales.sql`:

```bash
docker compose -f src-tauri/tests/databases/compose.yml up -d --wait
export MCFORGE_TEST_POSTGRES_URL=postgres://mcforge:mcforge@localhost:54329/sales
export MCFORGE_TEST_MYSQL_URL=mysql://mcforge:mcforge@localhost:33069/sales
cargo test --manifest-path src-tauri/Cargo.toml database
```

The same URLs work in the app's setup wizard (step *Connections* → *Test connection*). For SQLite, create a file with `sqlite3 lab/sales.db < src-tauri/tests/databases/sales.sql`. Stop the servers with `docker compose -f src-tauri/tests/databases/compose.yml down`.

## Before opening a PR

```bash
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo clippy --manifest-path src-tauri/Cargo.toml -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
```

By contributing you agree that your work is licensed under Apache-2.0 and you follow our [Code of Conduct](CODE_OF_CONDUCT.md).
