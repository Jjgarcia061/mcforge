# Adding a connector

A connector is one TOML file in `catalog/connectors/`. No Rust or TypeScript needed.

## 1. Try it in the lab

Run the MCP server by hand inside `lab/` (git-ignored) and list its tools, for example with the [MCP Inspector](https://github.com/modelcontextprotocol/inspector):

```bash
npx @modelcontextprotocol/inspector npx -y <package>
```

Only add servers that are actively maintained.

## 2. Write the manifest

```toml
id = "my-service"            # unique, kebab-case
name = "My Service"
category = "apps"            # files | data | reports | web | development | apps
description = "One sentence a non-technical person understands."
homepage = "https://github.com/…"
verified = false             # maintainers flip this after review

[server]
runtime = "npx"              # npx | uvx | binary | http | builtin
package = "@scope/my-service-mcp"
args = ["--workspace", "{{workspace}}"]

[server.env]
MY_SERVICE_TOKEN = "{{token}}"

[[fields]]                   # becomes a form in the wizard
key = "token"
label = "API token"
kind = "secret"              # text | secret | number | url | path | paths | select
required = true

[[fields]]
key = "workspace"
label = "Workspace"
kind = "text"
required = true

[[tools]]                    # declare EVERY tool the server exposes
name = "list_items"
risk = "read"                # read | write | destructive | network | send
```

For a remote server use `runtime = "http"` with `url` and optional `[server.headers]`.

## 3. Risk levels

| risk | meaning | default |
|---|---|---|
| `read` | only reads data | allow |
| `write` | creates or changes data | ask |
| `network` | reaches the internet | ask |
| `send` | acts as the user towards others (email, comment, PR) | ask |
| `destructive` | deletes, moves or resets | deny |

When in doubt, pick the stricter level.

## 4. Check it

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

The catalog tests reject unknown keys, duplicate ids or tools, `{{placeholders}}` without a matching field, and select fields without options. To have the connector preselected for a goal, add its id to `catalog/use-cases.toml`.
