# Security Policy

MCForge gives AI models access to your files, databases and apps, so security reports are our top priority.

## Reporting a vulnerability

Please **do not open a public issue**. Use GitHub's private reporting instead: *Security → Report a vulnerability* on this repository. We aim to acknowledge reports within 72 hours.

Especially relevant:
- Ways for a model or a tool result to bypass the permission gate (`allow` / `ask` / `deny`, path or read-only scopes).
- API keys or credentials leaving the OS keychain, being logged, or being sent to the wrong host.
- A catalog manifest that can execute something other than what it declares.

## Design commitments

- Permissions are enforced by the MCForge host on every tool call — never delegated to the model's prompt.
- API keys are stored in the OS keychain (macOS Keychain, Windows Credential Manager, Secret Service on Linux).
- Every tool call is written to a local audit log the user can inspect.
- Third-party MCP servers are external code; the UI always says so before installing one.
