# Architecture

MCForge is two layers with one boundary between them.

```
React UI (src/)  ──  typed IPC (src/lib/bindings.ts, generated)  ──  Rust core (src-tauri/src/)
                                                                       ├─ catalog    TOML manifests → validated data
                                                                       ├─ hardware   RAM / CPU / GPU detection
                                                                       ├─ recommend  hardware × models → ranking
                                                                       ├─ providers  Anthropic, OpenAI, Ollama
                                                                       ├─ database   built-in PostgreSQL / MySQL / SQLite connector
                                                                       ├─ policy     allow / ask / deny gate
                                                                       ├─ secrets    OS keychain
                                                                       └─ commands   the only IPC surface
```

## Rules that keep it lean

1. **Connectors are data.** A new integration is a TOML file in `catalog/connectors/`. The same engine renders its form, launches its server and enforces its permissions.
2. **One source of truth for types.** Rust structs derive `specta::Type`; `src/lib/bindings.ts` is generated from them (`cargo test` regenerates it, CI fails if it is stale). UI code imports through `src/lib/api.ts` only.
3. **The host enforces permissions.** Every tool declares a `risk`; the default permission is derived from it (`policy.rs`). The model is never trusted to respect rules written in a prompt.
4. **Secrets never touch disk.** Keys go to the OS keychain and are validated before they are stored.
5. **Experiments stay in `lab/`.** It is git-ignored. Code moves into the project only with a test.

## Data flow of the setup wizard

1. On start the core loads the catalog from the bundled `catalog/` resource folder and detects the hardware.
2. `system_report` ranks local models for this machine (`recommend.rs`).
3. `ollama_status` / `pull_model` manage local models through Ollama's HTTP API; download progress streams over a Tauri channel.
4. `connect_provider` validates an API key by listing models, then stores it in the keychain. `saved_provider_models` reuses a stored key.
5. Use cases (`catalog/use-cases.toml`) preselect connectors; each connector's `fields` become a form and its `tools` become permission toggles.
6. `test_connection` checks a connector's values before saving. Today the built-in database connector supports it.

## Database connector

`database.rs` exposes two types so the compiler keeps reads and writes apart:

- `Database` (read-only): every call runs inside a read-only transaction opened by the database itself (PostgreSQL `BEGIN READ ONLY` plus a read-only session default; MySQL read-only session and transaction; SQLite file opened read-only). Results are capped at 500 rows and 30 seconds. Tests check that `INSERT`, `UPDATE`, `DELETE`, `DROP`, `CREATE` and attempts to switch the session back to read-write all fail on the three engines.
- `WritableDatabase`: only for the `execute` tool, which defaults to *ask* in the permission gate.

For extra safety, connect with a database user that only has the privileges you want the AI to have.

## Roadmap

| Phase | Scope |
|---|---|
| F0 | Structure, catalog, hardware & recommendation, setup wizard |
| F1 | Chat with streaming and tool calling (Anthropic, OpenAI, Ollama) |
| F2 | MCP host: launch servers, enforce policy, audit log, activity view; persist setup |
| F3 | Report engine (PDF via Typst, XLSX, DOCX, HTML) + example templates |
| F4 | Larger catalog, export config to other MCP clients, signed installers |
