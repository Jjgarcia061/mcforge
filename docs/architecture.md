# Architecture

MCForge is two layers with one boundary between them.

```
React UI (src/)  ──  typed IPC (src/lib/bindings.ts, generated)  ──  Rust core (src-tauri/src/)
                                                                       ├─ catalog    TOML manifests → validated data
                                                                       ├─ hardware   RAM / CPU / GPU detection
                                                                       ├─ recommend  hardware × interview answers × models → one model to download
                                                                       ├─ providers  Anthropic, OpenAI, Ollama (keys, models, JSON answers)
                                                                       ├─ database   built-in PostgreSQL / MySQL / SQLite connector
                                                                       ├─ sources    CSV and spreadsheet files → cached read-only SQLite tables
                                                                       ├─ reports    plan → query → explain → render (MiniJinja + SVG charts)
                                                                       ├─ library    folders → passages, full-text + vector search, answers with citations, file map
                                                                       ├─ chat       conversations with tool calls under the permission gate
                                                                       ├─ setup      saved setup (JSON) with secrets in the keychain
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
2. `system_report` describes the machine and how much memory models can use on it.
3. The interview (`catalog/use-cases.toml`): the user picks tasks, a language and speed or quality. Each task weighs skills that every local model is rated on in `catalog/models.toml` (tools, analysis, code, Spanish). `recommend_models` (`recommend.rs`) scores the models that run on this machine, prefers a model already installed when it is nearly as good, adds the library's search model when a task needs it, and checks free disk space, so one download covers everything chosen.
4. `ollama_status` / `pull_model` manage local models through Ollama's HTTP API; download progress streams over a Tauri channel.
5. `connect_provider` validates an API key by listing models, then stores it in the keychain. `saved_provider_models` reuses a stored key.
6. The chosen tasks preselect connectors; each connector's `fields` become a form and its `tools` become permission toggles.
7. `test_connection` checks a connector's values before saving. Today the built-in database connector supports it.
8. `save_setup` writes the setup, interview answers included, to the app's config folder; `secret` field values go to the keychain instead. `load_setup` fills them back in, and the app opens on the workspace from then on.

The **Models** screen (`local_models`, `delete_model`) lists what Ollama has downloaded, switches the model in use and deletes the rest; it never deletes the model in use or the library's search model.

## Reports

`inspect_source` imports a CSV file or a workbook sheet (or reuses its cached import) and describes its columns. `generate_report` streams its stages over a Tauri channel while it plans, queries, explains and renders; see [report-templates.md](report-templates.md). `cancel_report` stops it at any stage: the request to the model is closed and no file is left behind, because imports and reports are only moved into place once complete. The AI's SQL runs through the same read-only `Database` as the database connector, so a model that writes `DELETE` gets an error, not a deletion. Every report is added to a history (`reports.json` in the app data folder, newest first, at most 200) that `list_reports` returns. Reports are shown in an in-app viewer (`read_report`, rendered in a sandboxed iframe where no script runs); `read_report` and `open_file` only accept files listed there, so the UI cannot be used to read or open arbitrary paths.

## Library

The library indexes folders the user picks (`.md`, `.txt`, `.html`, `.docx`, `.pdf`) into `library.db` in the app data folder; it never writes to those folders.

- **Update.** `library_status` compares the folders with the index (size and date only, so it is cheap) and reports new, changed and removed files. `update_library` reads only those, splits their text into ~1 200-character passages with some overlap, and stores each file in one transaction, so `cancel_library_update` can stop it at any point. A file whose content hash did not change keeps its passages.
- **Search.** Passages are searchable by words at once (SQLite FTS5, accent-insensitive). When Ollama has the embedding model from `catalog/models.toml`, they also get vectors and search becomes hybrid: both rankings are fused (reciprocal rank fusion). Passages indexed while the model was unavailable are embedded on the next update; a different model re-embeds everything.
- **Answers.** `ask_library` gives the model the best passages, numbered, and asks for an answer that cites them as `[n]`; citations to anything else are dropped.
- **Map.** After each update, `graph.rs` links every file to its closest files (by vector, or by shared words without one) and to the files it links in Markdown (`[[Note]]`, `[text](file.md)`), then groups them into topics with Louvain, named after their most distinctive words. `library_graph` returns it.
- `open_file` also opens indexed files; anything else is refused.
- **Drawing.** The Library screen draws the map with `d3-force` on a canvas (`src/features/library/graphView.ts`). Names appear only around the file under the cursor. From about 80 files a force starts holding the files inside a side-view outline of a human brain, and from 300 on they fill it, each of the eight largest topics settling where a lobe would be.

The PDF parser can panic on documents it does not support. Releases unwind instead of aborting so that `catch_unwind` turns that into one unreadable file rather than a crash.

## Chat

`providers/stream.rs` streams answers with tool calls from the three providers (Anthropic and OpenAI over server-sent events, Ollama as JSON lines) behind one `ChatModel` trait; a small decoder per provider turns its format into text deltas and complete tool calls.

`chat::run_turn` is the agent loop: it sends the conversation and the tools, and for every tool call asks `Tools::permission` before anything runs. *Allow* runs it, *deny* returns an error to the model, *ask* emits `ApprovalNeeded` and waits for `answer_approval` (allow once, allow always, which saves the rule in the setup, or reject). Results go back to the model until it answers or 8 steps pass. Every call is written to `activity.json` in the app data folder.

The tools (`chat/tools.rs`) are MCForge's own modules, offered only when set up: `library_search`, the database connector's `list_tables`, `describe_table`, `query` (read-only, enforced by the database) and `execute`, and the reports connector's `list_templates` and `render_report`, which can only read files the user attached to that conversation. Tool output is clipped for small context windows, and older tool output is shortened when the conversation grows.

Conversations are saved as JSON in `chats/`, before and after every turn; `stop_chat` cancels a turn and the saved conversation keeps what it finished, with any unanswered tool call marked as cancelled.

## Database connector

`database.rs` exposes two types so the compiler keeps reads and writes apart:

- `Database` (read-only): every call runs inside a read-only transaction opened by the database itself (PostgreSQL `BEGIN READ ONLY` plus a read-only session default; MySQL read-only session and transaction; SQLite file opened read-only). Results are capped at 500 rows and 30 seconds. Tests check that `INSERT`, `UPDATE`, `DELETE`, `DROP`, `CREATE` and attempts to switch the session back to read-write all fail on the three engines.
- `WritableDatabase`: only for the `execute` tool, which defaults to *ask* in the permission gate.

For extra safety, connect with a database user that only has the privileges you want the AI to have.

## Roadmap

| Phase | Scope |
|---|---|
| F0 | Structure, catalog, hardware & recommendation, setup wizard, saved setup, CSV and spreadsheet reports (HTML) |
| F1 | Chat with streaming and tool calling (Anthropic, OpenAI, Ollama) over MCForge's own tools — done |
| F2 | MCP host: launch servers, enforce policy, audit log, activity view; persist setup |
| F3 | More report formats (PDF via Typst, XLSX, DOCX), database sources, more templates |
| F4 | Larger catalog, export config to other MCP clients, signed installers |
