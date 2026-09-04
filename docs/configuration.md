# Configuration

[English](./configuration.md) | [简体中文](./configuration.zh-Hans.md) | [繁體中文](./configuration.zh-Hant.md) | [日本語](./configuration.ja.md) | [Русский](./configuration.ru.md)

`devo onboard` is the recommended setup path. For manual configuration, Devo
merges settings in this order:

1. Built-in defaults
2. `DEVO_HOME/config.toml` - user-level app config, defaulting to `~/.devo/config.toml`
   on macOS/Linux and `C:\Users\yourname\.devo\config.toml` on Windows
3. `DEVO_HOME/providers.json` - user provider connections and model selections
4. `<workspace>/.devo/config.toml` - project-level app config
5. `<workspace>/.devo/providers.json` - project provider/model overlay
6. CLI flags

Provider API keys live in the user-scoped `auth.json`. `providers.json` stores
only the credential id that points to the secret; the catalog itself is safe to
track and share.

Minimal shape (built-in catalog model + provider connection):

```json
{
  "model": "deepseek/deepseek-v4-flash",
  "provider": {
    "deepseek": {
      "name": "DeepSeek",
      "base_url": "https://api.deepseek.com/anthropic",
      "credential": "deepseek_api_key",
      "wire_api": "anthropic_messages",
      "models": {
        "deepseek-v4-flash": {
          "name": "DeepSeek V4 Flash"
        }
      }
    }
  }
}
```

The stable model identity is always `provider/model`: the provider map key is
the provider id and the nested model map key is the provider-facing model id.
There is no separate binding id, local slug, request model, or model
description to keep synchronized. Provider and model metadata are optional;
the tracked catalog supplies defaults and user/workspace files can add or
override entries.

Provider connections and model catalog entries are kept together in the JSON
shape. Secrets remain in the separate user-scoped auth file:

```json
{
  "provider": {
    "local": {
      "name": "Local Gateway",
      "base_url": "http://127.0.0.1:8000/v1",
      "wire_api": "openai_chat_completions",
      "credential": "local_gateway_api_key",
      "models": {
        "my-model": {
          "name": "My Model",
          "context_window": 131072
        }
      }
    }
  },
  "model": "local/my-model"
}
```

The tracked built-in directory is `crates/core/providers.json`. It is packaged
into Devo and should be changed through git. `DEVO_HOME/providers.json` and
`<workspace>/.devo/providers.json` are user/workspace overlays and may define
arbitrary custom providers and models.

The top-level `model` is the default primary `provider/model` used for normal
turns. The optional `small_model` is used for lightweight background work such
as session-title generation. When it is absent, Devo first looks for a
recognizably lightweight model in the same provider (for example `flash`,
`nano`, `haiku`, `mini`, `small`, or a small parameter-count model), then falls
back to the primary model. An invalid explicit `small_model` is ignored and
follows the same automatic fallback. The built-in directory leaves both
selections unset so it does not choose a provider for the user.

The directory currently includes Kimi (`kimi-k3`, `kimi-k2.7-code`,
`kimi-k2.6`), Z.ai and China BigModel/Zhipu AI (each with `glm-5.3` and
`glm-5.3-flash`),
DeepSeek, Qwen, MiniMax,
Xiaomi MiMo, and Tencent Hunyuan. DeepSeek uses its official Anthropic-compatible
endpoint by default. It also includes a local `ollama` provider template
(`http://localhost:11434/v1`) with an empty model directory; after connecting,
use Discover (Ollama `/api/tags` or OpenAI-compatible `/v1/models`) to load
models installed locally.

The catalog is a curated starting point, not a closed allowlist. Add any
provider or model by repeating the same nested shape in a user or workspace
`providers.json` overlay.

### Provider templates and Connections

Providers in the directory are read-only templates, not logged-in providers.
An embedded provider supplies its name, default base URL, wire API, and model
directory. Confirming it in onboarding creates a user Connection in the
user-level `providers.json`:

- Selecting an unconnected built-in provider opens the Connection settings
  page. The template Base URL is the default and can be overridden before
  connect; enter an API key (when required) to create the Connection. The key
  is stored in `auth.json`.
- Selecting an already Connected built-in provider opens that Connection's
  saved model list. Choose an existing model to configure it, use the custom
  model card to add another model, or select a saved model and press d/Delete
  to remove it from this Connection. The provider template and its built-in
  directory remain unchanged.
- A Connected provider's API key and Base URL cannot be edited from this flow.
  To replace the key or endpoint, disconnect the Connection and connect the
  template again.
- Selecting a custom provider opens editable Connection settings, because its
  name, endpoint, protocol, models, and credential are user-owned.
- The provider picker marks user Connections as `Connected` and untouched
  directory entries as `Template`. Select a Connected entry and press `d` or
  `Delete` to confirm disconnection. This removes the user provider overlay
  and a credential that is not shared by another Connection; the built-in
  template remains available.
- `Add custom provider` creates a custom Connection. Its provider id,
  endpoint, protocol, models, and credential are user-owned. Disconnect it
  with the same `d`/`Delete` action; do not remove the tracked directory.

The provider directory and provider Connections are therefore separate
concepts: the directory can be tracked with git, while Connections and
`auth.json` remain user configuration.

The old TOML provider shape remains readable for migration, but all new
onboarding and provider/model writes use `providers.json`.

Legacy equivalent (read-only compatibility):

```toml
[providers."api.deepseek.com/anthropic"]
enabled = true
name = "api.deepseek.com/anthropic"
base_url = "https://api.deepseek.com/anthropic"
credential = "api_deepseek_com_anthropic_api_key"
wire_apis = ["anthropic_messages"]
```

## Bring Your Own API Key

Put the credential id in `providers.json` and the actual key in the user-scoped
`auth.json`:

```json
{
  "provider": {
    "my-provider": {
      "base_url": "https://api.example.com/v1",
      "credential": "my_provider_api_key",
      "models": {
        "my-model": {"name": "My Model"}
      }
    }
  },
  "model": "my-provider/my-model"
}
```

`~/.devo/auth.json` (or `C:\Users\yourname\.devo\auth.json` on Windows):

```json
{
  "version": 1,
  "credentials": {
    "my_provider_api_key": {
      "kind": "api_key",
      "value": "sk-your-key"
    }
  }
}
```

`devo onboard` and the Desktop/TUI provider flows write both files. The
credential id must match exactly in the two files. Do not put `apiKey`,
`api_key`, or a secret in `providers.json`; those fields are not part of the
canonical provider schema. `auth.json` is user-scoped and must not be committed.

`auth.json` fields are:

| Field | JSON type | Meaning |
| --- | --- | --- |
| `version` | integer | Credential file schema version; currently `1`. |
| `credentials` | object | Map of credential ids to credential records. |
| `credentials.<id>.kind` | enum | Currently only `api_key` is supported. |
| `credentials.<id>.value` | string | The secret API key. |

If onboarding receives an API key without an explicit credential id, it uses a
stable id based on the provider id, such as `deepseek_api_key`. A missing
`auth.json` behaves like an empty credential file; a referenced but missing id is
an error.

### End-to-end example: custom model + your API key

The following pairs a custom DeepSeek model (Anthropic Messages), a provider
endpoint, and an API key stored in `auth.json`.

`~/.devo/providers.json` (or `C:\Users\yourname\.devo\providers.json` on Windows):

```json
{
  "model": "deepseek/my-deepseek",
  "provider": {
    "deepseek": {
      "name": "DeepSeek Anthropic Compatible",
      "base_url": "https://api.deepseek.com/anthropic",
      "credential": "deepseek_api_key",
      "wire_api": "anthropic_messages",
      "models": {
        "my-deepseek": {
          "name": "DeepSeek V4 Flash",
          "channel": "Custom",
          "context_window": 200000,
          "effective_context_window_percent": 95,
          "max_tokens": 8192,
          "temperature": 0.2,
          "reasoning_capability": {"togglewithlevels": ["high", "max"]},
          "reasoning_implementation": "request_parameter",
          "input_modalities": ["text"]
        }
      }
    }
  }
}
```

`~/.devo/auth.json`:

```json
{
  "version": 1,
  "credentials": {
    "deepseek_api_key": {
      "kind": "api_key",
      "value": "sk-deepseek-your-api-key"
    }
  }
}
```

Rules:

- `provider.<id>.credential` is the reference; the secret value belongs only in
  `auth.json` under the same id.
- Keep `auth.json` out of git. The tracked built-in `crates/core/providers.json`
  contains no user credentials.

## Provider/model JSON reference

The canonical file is JSON. Its root object has these fields:

### Root fields

| Field | JSON type | Default | Meaning |
| --- | --- | --- | --- |
| `model` | string | first enabled model | Active model in `provider/model` form. |
| `small_model` | string | automatic same-provider model, then `model` | Lower-cost model for lightweight background work, such as session titles. Invalid values use the same fallback. |
| `reasoning_effort` | string | model default | Global logical selection: `default`, `off`, `on`, or one of the effort values supported by the selected model. Legacy `disabled`/`enabled` normalize to `off`/`on`. |
| `provider` | object | `{}` | Map of provider ids to provider records. The canonical key is singular `provider`; `providers` is accepted only as a read compatibility alias. |

All fields inside a provider record and model record are optional. A selected
custom model can therefore start with only its map key; Devo supplies safe
runtime defaults. The provider id and model id are map keys, not duplicated
fields:

```json
{
  "model": "my-provider/my-model",
  "provider": {
    "my-provider": {
      "models": {
        "my-model": {}
      }
    }
  }
}
```

### Provider fields

Provider records live at `provider.<provider-id>`:

The `<provider-id>` map key is the stable provider identity used in
`provider/model`; `name` is only its display label. Renaming a provider should
not change its id.

| Field | JSON type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | provider id | Display name for the provider. |
| `base_url` | string | none | API endpoint base URL. Use the endpoint form required by the selected wire API. |
| `credential` | string | none | Credential id looked up in the user-scoped `auth.json`. The secret value is not stored in `providers.json`. |
| `headers` | object of string-to-string | none | Literal HTTP headers sent to the provider. Do not put API keys here. |
| `options` | JSON object | none | Provider-specific options. Object keys are forwarded to the built-in adapter request body unless a more specific model/variant value overrides them. |
| `request` | JSON object | none | Provider-level request-body defaults. Merged recursively before model and variant values. |
| `wire_api` | enum | `openai_chat_completions` | Default request protocol for all models in this provider. |
| `enabled` | boolean | `true` | Whether the provider and its models can be selected. |
| `env` | array of strings | `[]` | Environment variable names that integrations may use for provider credentials. |
| `web_search` | object | none | Provider-level web-search capability configuration; see the modes below. |
| `web_fetch` | object | none | Provider-level URL-fetch capability configuration; see the modes below. |
| `models` | object | `{}` | Map of provider-facing model ids to model records. |

Provider `headers` is a JSON object whose keys and values are both strings, for
example `{ "X-Organization": "my-team" }`.
`env` records names for integrations; the normal provider resolver does not
automatically read those names as API keys.

Inside a provider record, web-search and URL-fetch configuration uses these
fields:

```json
{
  "web_search": {
    "mode": "provider"
  },
  "web_fetch": {
    "mode": "local"
  }
}
```

`web_search.mode` is `disabled`, `provider`, or `local`; its optional
`local_provider` selects a named local search service and `local_providers`
defines those services. `web_fetch.mode` is `disabled`, `provider`, or `local`.
The default search mode is `provider`; the default fetch mode is `local`.

### Model fields

Models live at `provider.<provider-id>.models.<model-id>`. The nested map key
is the model id sent to the provider and is also the second half of the public
`provider/model` reference. There is intentionally no `model_slug`,
`model_name`, `model_id`, or model `description` field in the canonical format.

| Field | JSON type | Default | Meaning |
| --- | --- | --- | --- |
| `name` | string | model id | Human-readable label in the model picker. |
| `wire_api` | enum | provider `wire_api` | Request protocol override for this model. |
| `context_window` | integer | runtime default | Maximum context window in tokens. |
| `effective_context_window_percent` | number | runtime default | Percentage of `context_window` treated as usable (may be fractional). |
| `max_tokens` | integer | runtime default | Default response-output limit. |
| `temperature` | number | none | Sampling randomness. |
| `top_p` | number | none | Nucleus-sampling probability mass. |
| `top_k` | number | none | Candidate-token cap. |
| `reasoning_capability` | enum or object | `unsupported` | Reasoning choices shown to the user. |
| `reasoning_implementation` | enum or object | `request_parameter` when reasoning is supported | How the selected reasoning choice changes the request. |
| `default_reasoning_effort` | enum | none | Initial effort for level-capable reasoning. |
| `base_instructions` | string | built-in/default instructions | Model-specific base instructions. An explicit empty string disables them. |
| `input_modalities` | array of enums | `["text"]` | Accepted input types: `text` and/or `image`. |
| `channel` | string | none | Optional grouping label in the model picker. |
| `truncation_policy` | object | `{"mode":"bytes","limit":8000}` | Limit for oversized tool-result content. |
| `supports_image_detail_original` | boolean | `false` | Whether original-resolution image detail is supported. |
| `enabled` | boolean | `true` | Whether this model can be selected. |
| `priority` | integer | `0` | Higher values are listed/preferred first when no explicit model is selected. |

Additional model metadata and request controls are available:

| Field | JSON type | Meaning |
| --- | --- | --- |
| `family` | string | Model family used for grouping and future capability heuristics. |
| `release_date` | string | Provider catalog release date, normally ISO-8601 text. |
| `status` | string | Provider-reported availability label, for example `active`, `deprecated`, or `preview`. |
| `cost` | object | Open-ended pricing metadata; Devo preserves it without interpreting provider-specific keys. |
| `metadata` | object | Open-ended catalog metadata, including fields returned by dynamic discovery. |
| `options` | object | Arbitrary provider/SDK options. These are merged into request defaults for the built-in HTTP adapters. |
| `request` | object | Arbitrary request-body fields. These override the same model/provider option keys. |
| `headers` | object of string-to-string | Per-model HTTP headers layered over provider headers. API keys still belong in `auth.json`. |
| `variants` | object | Named variant map. For effort encodings, keys should be logical selections (`off`/`on`/levels). Each value may contain `label`, `disabled`, `request_model`, `options`, `request`, and `headers`. |
| `default_variant` | string | Variant key applied when a turn does not choose a variant explicitly and no effort-keyed variant matches. |

Variants use this shape:

```json
{
  "models": {
    "reasoning-model": {
      "family": "reasoning",
      "variants": {
        "fast": {
          "label": "Fast",
          "options": {"thinking": {"budget": 1024}},
          "request": {"speed": "fast"},
          "headers": {"X-Mode": "fast"}
        }
      },
      "default_variant": "fast"
    }
  }
}
```

The merge order is provider `options` → provider `request` → model
`options` → model `request` → variant `options` → variant `request`.
Headers use the same specificity order. `disabled: true` keeps a variant in
the directory for reproducibility but prevents it from being selected by a
future variant-aware client.

The effective context formula is
`context_window * effective_context_window_percent / 100`. That value is the
**applied** usable window for occupancy and auto-compact (default percent is
95 when unset).

Keep these two numbers distinct:

| Concept | Where you edit it | Example (DeepSeek V4 Flash) |
| --- | --- | --- |
| Model hard `context_window` | Catalog / discovery (not edited as a separate knob) | `1000000` (1M) |
| Usable Context window | Desktop / TUI model settings → **Context window** | User enters `250000` → stored as percent `25` → applied `250000` |

Desktop / TUI model editors show an absolute token count for the usable window.
On save, the hard `context_window` is left unchanged and
`effective_context_window_percent` is set to
`clamp(user_tokens × 100 / hard, 1..=100)` (fractional values allowed). Clearing the field removes
the percent overlay so the default 95% applies again. For a custom model with
no hard window yet, entering a value sets `context_window` to that amount and
percent to 100.

A legacy `compaction_token_limit` in `config.toml` is still loaded for
compatibility but is **ignored** for occupancy and auto-compact. There is no
separate Auto-compact threshold UI.

### Wire API values

`wire_api` may be set on a provider or model. A model value overrides its
provider value. These are the only supported values:

| Value | Request family | Use when |
| --- | --- | --- |
| `openai_chat_completions` | OpenAI-compatible Chat Completions | The endpoint accepts chat-completions requests. |
| `openai_responses` | OpenAI-compatible Responses | The endpoint accepts Responses API requests. |
| `anthropic_messages` | Anthropic-compatible Messages | The endpoint accepts Anthropic Messages requests. |

If omitted, Devo uses `openai_chat_completions`. Choose the value from the
provider's API documentation; the URL alone does not determine the protocol.

### Dynamic model discovery

The Native `provider/discover` method refreshes the model directory for an
existing Connection. It reads the credential referenced by `credential` from
the user-scoped `auth.json`, then tries the Connection base URL's `/models`
endpoint and compatible `/v1/models` forms. A successful OpenAI-style
`{"data":[...]}` or provider-style `{"models":[...]}` response is normalized
into the model map and persisted to the user `providers.json` overlay. Pass
`{"forceRefresh":true}` to bypass Devo's short in-process cache.

Discovery is additive: it updates the returned model records and preserves the
raw provider entry in `metadata`, while the git-tracked built-in directory is
never modified. Common `id`, `name`, `family`, `status`, release date, context
limit, output limit, cost, reasoning, and input-modality fields are normalized
when present. Providers without a model endpoint can use an explicit custom
`models` map instead.

### Reasoning fields

`reasoning_capability` controls which choices the UI exposes. It has exactly
`reasoning_capability` uses three JSON forms:

| JSON value | Meaning |
| --- | --- |
| `"unsupported"` | Do not expose reasoning controls. |
| `"toggle"` | Expose `off` and `on`. |
| `{ "levels": ["off", "low", "high"] }` | Expose exactly the listed chips. Include `off` to allow disabling; omit `off` when reasoning cannot be turned off. |

Legacy `{"toggle_with_levels":[...]}` still reads and migrates to
`levels` with a leading `off`.

The allowed effort strings are `none`, `minimal`, `low`, `medium`, `high`,
`xhigh`, and `max`. The array should contain only values supported by the
provider model. `default_reasoning_effort` is one of the same effort strings;
it is not used with `unsupported`. `default_reasoning_selection` stores the
exact logical selection (`off`, `on`, or a level). Legacy literals
`disabled`/`enabled` are accepted on read and normalized to `off`/`on`.

Session and composer UIs always pick a **logical** selection from
`reasoning_capability`. How that selection is encoded on the wire is
**per model on a Connection**, so the same upstream model can differ across
deployments:

| Mode | When | Behavior |
| --- | --- | --- |
| Adapter | No catalog `variants` key matches the selection | Built-in adapters fill first-class `thinking` / `reasoning_effort` fields. |
| CatalogVariant | `variants` contains a key equal to the selection (`off`/`on`/levels; legacy `disabled`/`enabled` keys also match) | First-class thinking/effort fields are cleared; that variant’s `request` / `options` / `headers` / optional `request_model` are merged into the outbound request. |

Name variant keys after logical selections. Example custom gateway that encodes
effort only in JSON:

```json
{
  "reasoning_capability": {"levels": ["low", "medium", "high"]},
  "default_reasoning_selection": "medium",
  "variants": {
    "low": {"request": {"ext": {"effort": "L"}}},
    "medium": {"request": {"ext": {"effort": "M"}}},
    "high": {"request": {"ext": {"effort": "H"}}}
  }
}
```

Example slug switch via `request_model` (replaces legacy
`reasoning_implementation: model_variant`):

```json
{
  "reasoning_capability": "toggle",
  "variants": {
    "off": {"request_model": "deepseek-chat"},
    "on": {"request_model": "deepseek-reasoner"}
  }
}
```

`reasoning_implementation` is retained only for old TOML migration and is
projected into `variants` when the variants map is empty. New JSON
configuration should use `reasoning_capability` for the reasoning selector and
the named `variants` map for encodings. Desktop and TUI both author these
model fields; day-to-day pickers only show capability-derived chips.

Desktop SDK note: the chat composer’s synthetic `variants` list is the set of
logical effort option values (from `availableEfforts`), not the catalog
`variants` map. Catalog encodings stay on the model record.

`truncation_policy` uses this exact shape:

```json
{
  "truncation_policy": {
    "mode": "tokens",
    "limit": 12000
  }
}
```

`mode` is either `bytes` or `tokens`; `limit` is an integer.

### Overlay and custom-model rules

User and workspace files overlay the git-tracked directory in order. Repeating
the same provider/model key partially overrides only the fields present in the
higher-priority file. Adding a new provider key creates a custom provider;
adding a new nested model key creates a custom model with safe defaults. The
new model is selected by its `provider/model` reference:

```json
{
  "model": "example/custom",
  "provider": {
    "example": {
      "name": "Example Gateway",
      "base_url": "https://api.example.com/v1",
      "credential": "example_api_key",
      "wire_api": "openai_chat_completions",
      "models": {
        "custom": {
          "name": "Example Custom Model",
          "context_window": 128000,
          "input_modalities": ["text"]
        }
      }
    }
  }
}
```

Omitting model metadata is valid. Omitted built-in fields remain unchanged
when overriding a built-in entry; omitted custom-model fields use Devo's
defaults. The old TOML scalar, provider, binding, and model-override fields
remain readable only for migration.

### TUI Preferences

Top-level keys in `DEVO_HOME/config.toml` also store a few UI preferences:

```toml
theme = "aurora"
collapse_reasoning = true
```

- `theme` selects the TUI color theme (also set via Settings › Appearance).
- `collapse_reasoning` controls reasoning display (also set via `/show-reasoning`):
  - `true` (default): while streaming, show only the latest 3 lines; when finished, keep short
    reasoning in full and collapse longer reasoning to a one-line `Thought · …`
    summary (full text remains available in Ctrl+T).
  - `false`: show full reasoning while streaming and after it finishes.
- Legacy `compaction_token_limit`, if present, is ignored. Set each model's
  usable Context window in Settings › Models instead.

### Migrating provider settings from `config.toml`

The git-tracked `crates/core/providers.json` is Devo's built-in provider and
model directory. On startup, when Devo loads a user or workspace
`config.toml`, it automatically migrates legacy provider, model, binding, and
model-selection settings into the matching `providers.json` overlay before
resolving the active model:

- User settings move from `DEVO_HOME/config.toml` to
  `DEVO_HOME/providers.json`.
- Workspace settings move from `<workspace>/.devo/config.toml` to
  `<workspace>/.devo/providers.json`.
- Existing JSON values win over legacy TOML values, so a newer JSON
  configuration is never overwritten by an older one.
- Legacy API keys are copied to user-scoped `auth.json` and referenced from
  `providers.json` by credential id. API key values are never written to the
  provider catalog.
- Only provider-owned TOML keys are removed. Unrelated application settings
  remain in `config.toml`. A legacy `[model.<name>]` entry is kept when Devo
  cannot safely associate it with a provider model.

The migration is idempotent: after the first successful startup, subsequent
starts use the JSON catalog directly. New onboarding and provider/model writes
also use `providers.json`.

## MCP Servers

Devo connects to [Model Context Protocol](https://modelcontextprotocol.io/)
servers configured in user or workspace `config.toml` under `[mcp]`. Each server
is one entry in the `servers` array, and its `transport` table selects how Devo
connects. Supported transports are `stdio`, `streamable_http`, and the deprecated
`sse`.

You can configure MCP either by editing `config.toml` or with the CLI
(`devo mcp …`). Prefer the CLI for day-to-day add / enable / disable / remove;
edit TOML when you need transport details, env vars, or headers.

### Bundled `code_search` (disabled by default)

Devo ships an optional semantic search MCP binary next to `devo`. The config
entry is injected when missing and stays **disabled** until you enable it:

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

```bash
devo mcp enable code_search
# or, in an interactive session: /mcps → Code Search → Enable
```

When enabled, the model-facing tool name is `mcp__code_search__code_search`.
The `devo-code-search-mcp` binary is installed next to `devo`.

### CLI management

Manage user-level MCP servers (`~/.devo/config.toml`) with `devo mcp`:

```bash
# List configured servers (effective / user config)
devo mcp list

# Add a stdio server (command + args after --)
devo mcp add time -- docker run -i --rm mcp/time
devo mcp add filesystem --env HOME=/tmp -- npx -y @modelcontextprotocol/server-filesystem .

# Add Streamable HTTP (`--transport http` writes kind = "streamable_http")
devo mcp add --transport http hello-mcp http://localhost:8080/mcp
devo mcp add --transport http github --bearer-token "$TOKEN" https://api.githubcopilot.com/mcp/

# Add legacy SSE
devo mcp add --transport sse legacy-mcp https://example.com/mcp/sse

# Enable / disable / remove by server id
devo mcp enable time
devo mcp disable time
devo mcp remove time
```

CLI `devo mcp enable|disable` writes user `config.toml` for offline use. An
already-running interactive session applies enable/disable live through the TUI
`/mcps` path (`mcp/set_enabled` RPC).

Verify configuration in the TUI with `/mcps` (interactive server list → detail →
tools). Clients can also call `mcp/list`, `mcp/tools`, and `mcp/set_enabled`.

### TOML examples

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
