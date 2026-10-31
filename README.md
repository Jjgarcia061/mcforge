# MCForge

**Set up your own AI assistant with real tools — no JSON, no terminal.**

MCForge is an open-source desktop app (Windows, macOS, Linux) that walks you through building a local [Model Context Protocol](https://modelcontextprotocol.io) setup step by step:

1. **Checks your computer** and recommends a model that will actually run well on it.
2. **Pick your engine**: a local model (via [Ollama](https://ollama.com)) or an API (Anthropic, OpenAI).
3. **Pick what you need**: files, databases, web, Git, reports… from a growing catalog.
4. **Decide what the AI may do**: every tool is allowed, asks first, or is blocked. MCForge enforces it — not the prompt.
5. **Generate reports**: pick a CSV or Excel file and ask a question. The AI plans the analysis as read-only SQL and writes the conclusions; MCForge runs the queries, draws the charts and renders the document from a template, so every figure comes straight from the data.

> Status: early development (v0.1). The setup wizard and reports from CSV and Excel files work; chat, the MCP host and PDF / Excel / Word output are next.

## Why

Configuring MCP today means editing config files, installing runtimes and trusting servers blindly. MCForge makes it a guided, auditable process for everyone.

## Architecture in one paragraph

A Rust core (Tauri 2) does the heavy lifting — hardware detection, model recommendation, the MCP host, the permission gate, the report engine and OS-keychain storage for API keys. The UI is React + TypeScript. Connectors are **data, not code**: each one is a small TOML manifest in [`catalog/connectors`](catalog/connectors), so adding a new integration rarely needs a single line of Rust. See [docs/architecture.md](docs/architecture.md).

## Development

Requirements: [Rust](https://rustup.rs) (stable), Node.js ≥ 20, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/) for your OS.

```bash
npm install
npm run tauri dev
```

Checks:

```bash
npm run check   # typecheck + lint + UI tests
cargo test --manifest-path src-tauri/Cargo.toml
```

To tune the report prompts against a real local model:

```bash
MCFORGE_TEST_OLLAMA_MODEL=qwen3:8b MCFORGE_TEST_CSV=path/to/file.csv \
  cargo test --manifest-path src-tauri/Cargo.toml real_local_model -- --ignored --nocapture
```

The `lab/` folder is git-ignored: use it for experiments and local secrets, then move what works into the project with a test.

## Contributing

The easiest way to help is adding a connector — see [docs/adding-a-connector.md](docs/adding-a-connector.md) and [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[Apache-2.0](LICENSE)
