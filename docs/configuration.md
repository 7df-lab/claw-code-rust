# Configuration

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` is the recommended setup path. For manual configuration, Devo
merges settings in this order:

1. Built-in defaults
2. `DEVO_HOME/config.toml` - user-level config, defaulting to `~/.devo/config.toml`
   on macOS/Linux and `C:\Users\yourname\.devo\config.toml` on Windows
3. `<workspace>/.devo/config.toml` - project-level config
4. CLI flags

Credentials live separately in `DEVO_HOME/auth.json`; `config.toml` should refer
to credential ids instead of storing API keys directly.

Minimal shape (built-in model + provider binding):

```toml
[defaults]
model_binding = "deepseek-v4-flash-api-deepseek-com"

[providers."api.deepseek.com"]
enabled = true
name = "api.deepseek.com"
base_url = "https://api.deepseek.com"
credential = "api_deepseek_com_api_key"
wire_apis = ["openai_chat_completions"]

[model_bindings.deepseek-v4-flash-api-deepseek-com]
enabled = true
model_slug = "deepseek-v4-flash"
provider = "api.deepseek.com"
request_model = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"
```

The important separation is:

- `model_slug` selects Devo's local model metadata by slug.
- The binding's `provider` selects a `[providers.<id>]` connection record.
- `request_model` is the provider-facing model id sent on the wire.
- `invocation_method` selects the operational provider protocol. See
  [Invocation methods](#invocation-methods).

Model metadata also has a `provider` field. It describes the wire API the model
expects, while the binding's `invocation_method` chooses the connection used at
runtime; keep those values aligned. API keys remain in `auth.json` and are
connected through the provider's `credential` reference.

Existing configuration using `model_name` remains readable. Devo writes the
field as `request_model` the next time that binding is saved.

## Bring Your Own API Key

Devo does not store API keys in `config.toml`. When you bring your own key:

1. Store the secret in user-scoped `DEVO_HOME/auth.json`.
2. Point `[providers.<id>].credential` at that credential id from
   `config.toml`.

`devo onboard` and the Desktop/TUI provider flows write both files for you.

### End-to-end example: custom model + your API key

The following pairs a custom DeepSeek model (Anthropic Messages), a provider
endpoint, and a credential stored only in `auth.json`.

`~/.devo/config.toml` (or `C:\Users\yourname\.devo\config.toml` on Windows):

```toml
[defaults]
model_binding = "deepseek-example"

[model.my-deepseek]
display_name = "DeepSeek V4 Flash"
description = "Custom Anthropic Messages coding model for DeepSeek."
channel = "Custom"
# Wire API this model expects. Must match the binding's invocation_method.
provider = "anthropic_messages"
context_window = 200000
effective_context_window_percent = 95
max_tokens = 8192
temperature = 0.2
reasoning_capability = { togglewithlevels = ["high", "max"] }
reasoning_implementation = "request_parameter"
base_instructions = "(optional) You are Devo, a coding agent."
input_modalities = ["text"]
# For multimodalities
# input_modalities = ["text", "image"] ...

[providers.deepseek]
enabled = true
name = "DeepSeek Anthropic Compatible"
base_url = "https://api.deepseek.com/anthropic"
# Credential id only — the secret lives in auth.json.
credential = "deepseek_compatible_api_key"
wire_apis = ["anthropic_messages"]

[model_bindings.deepseek-example]
enabled = true
model_slug = "my-deepseek"
provider = "deepseek"
request_model = "deepseek-v4-flash"
display_name = "DeepSeek V4 Flash"
invocation_method = "anthropic_messages"
```

Matching `~/.devo/auth.json` (or `C:\Users\yourname\.devo\auth.json`):

```json
{
  "version": 1,
  "credentials": {
    "deepseek_compatible_api_key": {
      "kind": "api_key",
      "value": "sk-deepseek-your-api-key"
    }
  }
}
```

Rules:

- Only `api_key` credentials are supported today.
- The credential id must match `[providers.<id>].credential` exactly.
- Keep `auth.json` under `DEVO_HOME`. Do not commit it to a project repo.
- Workspace `<workspace>/.devo/config.toml` may reference credential ids, but
  secret values stay in user-scoped `auth.json`.
- Updating only the key means editing `auth.json`; leave `config.toml`
  unchanged when the credential id stays the same.

## Invocation methods

`invocation_method` (on a model binding) and `wire_apis` (on a provider) select
which HTTP API Devo uses for that connection. Model metadata `provider` should
use the same value so catalog capabilities match the runtime connection.

| Value | Protocol | Typical endpoints |
| --- | --- | --- |
| `openai_chat_completions` | [OpenAI Chat Completions](https://developers.openai.com/api/reference/chat-completions/overview) | Most OpenAI-compatible gateways (DeepSeek, Qwen, Kimi, OpenRouter, many local proxies) |
| `openai_responses` | [OpenAI Responses](https://developers.openai.com/api/reference/responses/overview) | Providers that expose the Responses API |
| `anthropic_messages` | [Anthropic Messages](https://platform.claude.com/docs/en/api/messages) | Anthropic-compatible Messages endpoints |

## Model Metadata and Custom Models

Configure model metadata in user or workspace `config.toml` under
`[model.<slug>]`. A section for a built-in slug is a partial override: omitted
fields retain their built-in values. A new slug creates a custom model with safe
defaults, which should then be connected through both `[providers.<id>]` and
`[model_bindings.<id>]` as in the
[end-to-end example](#end-to-end-example-custom-model--your-api-key).

For example, this changes only the built-in context window:

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

The exact effective context formula is
`context_window * effective_context_window_percent / 100`; the result is the
context available to the model and the automatic-compaction boundary.

Configurable metadata includes `display_name`, the picker-facing model name;
`description`, explanatory text shown to users; and `channel`, the grouping
label used to organize models. `context_window` and
`effective_context_window_percent` determine effective context, while
`max_tokens` is the default response-output limit. Sampling defaults are
`temperature` for randomness, `top_p` for nucleus probability mass, and `top_k`
for the candidate-token cap. The `provider` wire API is one of
`openai_chat_completions`, `openai_responses`, or `anthropic_messages`.
Reasoning metadata is typed: `reasoning_capability` can be `unsupported`,
`toggle`, `{ levels = [...] }`, or `{ togglewithlevels = [...] }`;
`reasoning_implementation` can be `disabled`, `request_parameter`, or a typed
`model_variant` table. A model variant maps a logical reasoning selection to a
different provider-facing model id, optional effective effort, and optional
extra request body instead of changing a parameter on the same model;
`default_reasoning_effort` selects the default typed effort. `input_modalities`
accepts `text` and `image`; `truncation_policy` chooses a byte or token limit for
oversized tool-result content before it is included in a model request; and
`supports_image_detail_original` enables original image detail.

Omitting `base_instructions` retains built-in instructions for a built-in model
or uses Devo's default instructions for a custom model. An explicit empty string
(`base_instructions = ""`) means no base instructions.

Legacy `model = "slug"` remains readable. Because `[model.<slug>]` now owns the
top-level `model` table namespace, new configuration must select the active
connection with `[defaults].model_binding` instead of the legacy scalar key.

### TUI Preferences

Top-level keys in `DEVO_HOME/config.toml` also store a few UI preferences:

```toml
theme = "aurora"
collapse_reasoning = true
```

- `theme` selects the TUI color theme (also set via `/theme`).
- `collapse_reasoning` controls reasoning display (also set via `/show-reasoning`):
  - `true` (default): while streaming, show only the latest 3 lines; when finished, keep short
    reasoning in full and collapse longer reasoning to a one-line `Thought · …`
    summary (full text remains available in Ctrl+T).
  - `false`: show full reasoning while streaming and after it finishes.

### Migrating from `models.json`

Old `~/.devo/models.json` and `<workspace>/.devo/models.json` files are ignored.
Manually copy the fields you still want into `[model.<slug>]` sections in the
user or workspace `config.toml`, then add or retain the matching provider and
model binding. Keep API keys in `auth.json`; refer to them from
`[providers.<id>].credential`.

## MCP Servers

Devo connects to [Model Context Protocol](https://modelcontextprotocol.io/)
servers configured in user or workspace `config.toml` under `[mcp]`. Each server
is one entry in the `servers` array, and its `transport` table selects how Devo
connects. Supported transports are `stdio`, `streamable_http`, and the deprecated
`sse`.

Devo also presets a bundled, disabled-by-default semantic search server:

```toml
[[mcp.servers]]
id = "code_search"
display_name = "Code Search"
enabled = false
startup_policy = "lazy"

[mcp.servers.transport]
kind = "stdio"
command = ["devo-code-search-mcp"]
```

Enable it with `devo mcp enable code_search` (or `/mcps` in the TUI). The
`devo-code-search-mcp` binary is installed next to `devo`.

Stdio example:

```toml
[mcp]
auto_start = true

[[mcp.servers]]
id = "filesystem"
display_name = "Filesystem"
enabled = true
startup_policy = "lazy" # eager | lazy | manual
trust_policy = "user" # user | workspace | untrusted
allowed_capabilities = ["tools", "resources", "prompts"]
roots_policy = "workspace" # none | workspace | custom

[mcp.servers.transport]
kind = "stdio"
command = ["npx", "-y", "@modelcontextprotocol/server-filesystem", "."]
# cwd = "/path/to/workdir"
# env = { MY_VAR = "value" }
# env_vars = ["HOME", "PATH"]
```

Streamable HTTP with a bearer token:

```toml
[[mcp.servers]]
id = "github"
display_name = "GitHub"
startup_policy = "lazy"

[mcp.servers.transport]
kind = "streamable_http"
url = "https://api.githubcopilot.com/mcp/"
auth = { kind = "bearer_token", token = "replace-me" }
http_headers = { "X-Custom" = "static-value" }
env_http_headers = { "Authorization" = "GITHUB_TOKEN" }
```

Legacy SSE transport:

```toml
[mcp.servers.transport]
kind = "sse"
url = "https://example.com/mcp/sse"
```

Field notes:

- `auto_start` defaults to `true`. Enable/disable of MCP servers in a running session is applied live via `mcp/set_enabled` (TUI `/mcps`).
- `startup_policy` controls when an enabled server starts: `eager` during
  bootstrap, `lazy` on first use, or `manual` only by explicit request.
- For stdio, `env` provides literal values and `env_vars` lists names inherited
  from the local environment; `{ name = "X", source = "remote" }` is not
  supported for stdio.
- For HTTP transports, `http_headers` provides literal headers and
  `env_http_headers` maps a header name to the environment variable that
  supplies its value.
- Empty `allowed_capabilities` means no restriction. The runtime currently
  focuses on `tools`; resource reads are not wired yet.
- `output_limits` sets `max_tool_output_bytes` (default 1 MiB) and
  `max_resource_bytes` (default 10 MiB).
- Top-level `mcp_oauth_credentials_store` is `auto` (default), `file`, or
  `keyring` and selects where OAuth credentials are stored.
- Prefer environment-injected headers or values over hard-coding tokens into
  `config.toml`. `auth_ref` exists on each server record but is not wired to the
  runtime yet.

Merge behavior: `[mcp]` is merged field-wise like other tables, but `servers` is
an array. A project-level `[[mcp.servers]]` list therefore replaces the
user-level list instead of merging by `id`.

### CLI management

Manage user-level MCP servers (`~/.devo/config.toml`) with `devo mcp`:

```bash
# Stdio (command + args after --)
devo mcp add time -- docker run -i --rm mcp/time
devo mcp add filesystem --env HOME=/tmp -- npx -y @modelcontextprotocol/server-filesystem .

# Streamable HTTP (`--transport http` writes kind = "streamable_http")
devo mcp add --transport http hello-mcp http://localhost:8080/mcp
devo mcp add --transport http github --bearer-token "$TOKEN" https://api.githubcopilot.com/mcp/

# Legacy SSE
devo mcp add --transport sse legacy-mcp https://example.com/mcp/sse

devo mcp list
devo mcp enable time
devo mcp disable time
devo mcp remove time
```

CLI `devo mcp enable|disable` writes user `config.toml` for offline use. An
already-running interactive session applies enable/disable live through the TUI
`/mcps` path (`mcp/set_enabled` RPC).

Verify configuration in the TUI with `/mcps` (interactive server list → detail →
tools; Enable/Disable persists config and applies the manager + tool registry
for the next turn). Clients can also call `mcp/list`, `mcp/tools`, and
`mcp/set_enabled`. Use `devo mcp add|list|remove|enable|disable` for CLI
management.
