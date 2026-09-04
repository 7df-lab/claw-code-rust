# Config

This crate owns Devo's file-backed runtime configuration.

Shared serializable config contract types that are consumed by multiple crates
also live here. Runtime interpretation of the resolved config stays in the
consumer crates.

## Module Map

- `lib.rs` re-exports the public config surface.
- `app.rs` defines `AppConfig`, its defaults, config loading, merge behavior,
  validation, project config keys, and `AppConfigStore`.
- `server.rs` defines server transport and connection defaults.
- `logging.rs` defines logging and rolling file-log settings.
- `skills.rs` defines skill discovery settings.
- `hooks.rs` defines external hook event and command configuration.
- `experimental.rs` defines opt-in experimental feature gates.
- `error.rs` defines app and provider config error types.
- `provider.rs` re-exports provider config APIs and contains provider-focused
  tests.
- `provider/` contains provider config schema, TOML/JSON persistence, auth
  storage, provider resolution, and the provider config store.
- `tests.rs` contains app-config loader tests.

## Config Files

The user-level application config file is `<DEVO_HOME>/config.toml`. Provider
connections and model selection live in the standalone
`<DEVO_HOME>/providers.json` file. `DEVO_HOME` defaults to `~/.devo`; if the
environment variable is set, it must point to an existing directory.

When a workspace is known, the project-level config file is:

```text
<workspace>/.devo/config.toml
```

When a workspace is known, its provider/model overlay is:

```text
<workspace>/.devo/providers.json
```

The canonical provider/model shape is intentionally small:

```json
{
  "model": "local/my-model",
  "provider": {
    "local": {
      "base_url": "http://127.0.0.1:8000/v1",
      "credential": "local_api_key",
      "wire_api": "openai_chat_completions",
      "models": {
        "my-model": {"name": "My Model", "context_window": 131072}
      }
    }
  }
}
```

Provider ids and model ids are map keys; the only model reference exposed to
users is `provider/model`. There is no persisted binding id, model slug, model
name, model id, or model description. `crates/core/providers.json` is the
git-tracked built-in directory. User and workspace `providers.json` files may
add or override arbitrary providers and models.

Provider API keys are stored in the user-scoped `auth.json`; `providers.json`
contains only the matching `credential` id. Do not commit `auth.json` or other
files containing real secrets. The old provider TOML tables remain readable as
migration compatibility paths, while new provider/model writes use JSON plus
the separate auth file.

## Load And Merge Order

`FileSystemAppConfigLoader` starts from `AppConfig::default()` and overlays
config in this order:

1. User config: `<DEVO_HOME>/config.toml`
2. Project config: `<workspace>/.devo/config.toml`
3. CLI overrides

Later layers win over earlier layers for overlapping fields. TOML tables are
merged recursively; non-table values replace the earlier value.

Provider-owned fields use `ProviderConfigSection::merge_overlay` while loading
user, project, and CLI config, so higher-priority layers can override specific
provider fields without clearing every omitted provider field from lower layers.

## App Defaults

`AppConfig::default()` currently sets:

- `summary_model = "UseTurnModel"`
- `server.listen = []`
- `server.max_connections = 32`
- `server.event_buffer_size = 1024`
- `server.idle_session_timeout_secs = 1800`
- `server.persist_ephemeral_sessions = false`
- `server.auth.enabled = false`
- `server.auth.method_id = "agent-login"`
- `server.auth.name = "Agent login"`
- `server.auth.description = None`
- `server.auth.logout = true`
- `logging.level = "info"`
- `logging.json = false`
- `logging.redact_secrets_in_logs = true`
- `logging.file.directory = None`
- `logging.file.filename_prefix = "devo"`
- `logging.file.rotation = "Daily"`
- `logging.file.max_files = 14`
- `skills.enabled = true`
- `skills.user_roots = ["skills"]`
- `skills.workspace_roots = ["skills"]`
- `skills.watch_for_changes = true`
- `skills.bundled.enabled = true`
- `skills.include_instructions = true`
- `skills.config = []`
- bundled `[[mcp.servers]]` entry `id = "code_search"` with `enabled = false`
- `tools.web_search.mode = "provider"`
- `updates.enabled = true`
- `updates.check_on_startup = true`
- `updates.check_interval_hours = 24`
- `hooks = {}`
- `project_root_markers = [".git"]`
- `projects = {}`

## App Config Shape

Top-level app config fields include:

```toml
summary_model = "UseTurnModel" # or "UseAxiliaryModel"
project_root_markers = [".git"]

[server]
listen = []
max_connections = 32
event_buffer_size = 1024
idle_session_timeout_secs = 1800
persist_ephemeral_sessions = false

[server.auth]
enabled = false
method_id = "agent-login"
name = "Agent login"
description = "Sign in using the agent"
logout = true

[logging]
level = "info"
json = false
redact_secrets_in_logs = true

[logging.file]
directory = "diagnostics"
filename_prefix = "devo"
rotation = "Daily" # Never, Minutely, Hourly, or Daily
max_files = 14

[skills]
enabled = true
user_roots = ["skills"]
workspace_roots = ["skills"]
watch_for_changes = true
include_instructions = true

[skills.bundled]
enabled = true

[[skills.config]]
path = "/path/to/skill/SKILL.md"
enabled = false

[[skills.config]]
name = "code-review"
enabled = true

[[mcp.servers]]
id = "code_search"
display_name = "Code Search"
enabled = false
startup_policy = "lazy"

[mcp.servers.transport]
kind = "stdio"
command = ["devo-code-search-mcp"]

[tools.web_search]
mode = "local" # disabled, provider, or local
local_provider = "exa"

[tools.web_search.local_providers.exa]
kind = "exa" # exa or tavily
credential = "exa_api_key"
max_results = 5

[updates]
enabled = true
check_on_startup = true
check_interval_hours = 24

[projects."/path/to/project"]
permission_preset = "auto-review" # default (ask), auto-review (product default when unset), or full-access
sandbox_profile = "workspace" # workspace, devbox, read-only, strict, off, or a custom profile from sandbox.toml

[[hooks.PreToolUse]]
matcher = "exec_command"

[[hooks.PreToolUse.hooks]]
type = "command"
command = "hooks/pre-tool-use.sh"
timeout = 30
```

`server.listen` accepts `stdio://` and `ws://host:port` entries. An empty list
uses the server defaults: stdio plus `ws://127.0.0.1:3210`. The short `ws://`
entry also binds to `127.0.0.1:3210`. To run only WebSocket transport from the
CLI, use `devo server --transport websocket`.

`logging.file.directory` is optional. Relative logging directories resolve under
`DEVO_HOME`.

## Validation

`validate_app_config` rejects configs when:

- `server.listen` contains duplicate endpoints.
- `server.auth.method_id` is empty or whitespace while server auth is enabled.
- `server.auth.name` is empty or whitespace while server auth is enabled.
- `logging.file.max_files` is less than `1`.
- `logging.file.filename_prefix` is empty or whitespace.
- `updates.check_interval_hours` is less than `1`.
- `skills.user_roots` contains duplicate paths.
- `skills.workspace_roots` contains duplicate paths.
- `skills.config` entries include both `path` and `name`.
- `skills.config` entries include neither `path` nor `name`.
- `skills.config` name selectors are empty.

Provider-specific validation happens while resolving or mutating provider
config.

## Hooks

External hooks are configured under the top-level `[hooks]` table. Each hook
event contains matcher entries, and each matcher entry contains one or more hook
commands:

```toml
[[hooks.PostToolUse]]
matcher = "exec_command|read_file"

[[hooks.PostToolUse.hooks]]
type = "command"
command = "hooks/post-tool-use.sh"
shell = "bash"
timeout = 30

[[hooks.UserPromptSubmit]]

[[hooks.UserPromptSubmit.hooks]]
type = "command"
command = "hooks/check-prompt.sh"
async = false
```

Command hooks receive one JSON object on stdin. The common fields are
`hook_event_name`, `session_id`, `transcript_path`, and `cwd`. Runtime contexts
may also include `permission_mode`, `agent_id`, and `agent_type`, followed by
event-specific fields such as `tool_name`, `tool_input`, `tool_use_id`,
`tool_response`, `prompt`, `source`, `trigger`, `reason`, `file_path`, `event`,
`old_cwd`, and `new_cwd`.

Hook command results follow Claude Code-style blocking semantics:

- Exit status `0` succeeds unless stdout contains a blocking JSON decision.
- Exit status `2` blocks the triggering action. The block reason is read from
  stdout JSON or stderr.
- Stdout JSON shaped as `{"decision":"block","reason":"..."}` blocks even when
  the process exits successfully.
- Claude-style `hookSpecificOutput` denial JSON blocks for `PreToolUse` and
  `PermissionRequest`.
- Stdout JSON shaped as `{"continue":false,"stopReason":"..."}` is treated as a
  blocking stop for lifecycle events that consume blocking decisions.
- Other non-zero exits are logged as non-blocking hook failures.

The `command` hook type is executed by the runtime. `prompt`, `agent`, and
`http` hook definitions are parsed so config files remain forward compatible,
but they are currently logged as unsupported and not executed. `shell` accepts
`bash` and `powershell`. `timeout` is in seconds and defaults to `600`.
`async = true` and `asyncRewake = true` spawn the command in the background and
do not wait for a blocking decision. `if`, `status_message`, and `once` are
preserved in config but are not interpreted by the current runtime.

All 27 hook event names are accepted by config:

- `PreToolUse`, `PostToolUse`, `PostToolUseFailure`
- `Notification`, `UserPromptSubmit`
- `SessionStart`, `SessionEnd`, `Stop`, `StopFailure`
- `SubagentStart`, `SubagentStop`
- `PreCompact`, `PostCompact`
- `PermissionRequest`, `PermissionDenied`
- `Setup`, `TeammateIdle`, `TaskCreated`, `TaskCompleted`
- `Elicitation`, `ElicitationResult`
- `ConfigChange`, `WorktreeCreate`, `WorktreeRemove`
- `InstructionsLoaded`, `CwdChanged`, `FileChanged`

The current runtime triggers hooks where Devo has a matching lifecycle point:
tool execution, prompt submission, server setup, session start and resume,
session shutdown, turn stop and failure, subagent start and stop, manual
compaction, permission request and denial, config writes through `provider/upsert`
and `skill/set_enabled`, per-turn cwd changes, and file changes reported by
`write`/`apply_patch` tool metadata.

Runtime-triggered events:

- `PreToolUse`, `PostToolUse`, `PostToolUseFailure`
- `UserPromptSubmit`
- `SessionStart`, `SessionEnd`
- `Stop`, `StopFailure`
- `SubagentStart`, `SubagentStop`
- `PreCompact`, `PostCompact`
- `PermissionRequest`, `PermissionDenied`
- `Setup`
- `ConfigChange`
- `CwdChanged`, `FileChanged`

Config-ready but not currently triggered:

- `Notification`: Devo has protocol notifications, but no single user-facing
  notification lifecycle equivalent to Claude's external notification hook.
- `TeammateIdle`, `TaskCreated`, `TaskCompleted`: the standalone `devo-tasks`
  crate is not wired into the server runtime task lifecycle.
- `Elicitation`, `ElicitationResult`: MCP elicitation is currently handled
  inside the MCP manager with an automatic response and no server-session hook
  bridge.
- `WorktreeCreate`, `WorktreeRemove`: Devo currently has no worktree lifecycle
  API.
- `InstructionsLoaded`: Devo discovers AGENTS-style instructions during context
  assembly, but does not expose a hookable per-file instruction-load event.

## Provider Config

The canonical provider config is standalone JSON and is modeled by
`ProviderConfigFile`. Provider ids and model ids are map keys, so the only
model reference exposed to users is `provider/model`:

```json
{
  "model": "main/gpt-5.4",
  "reasoning_effort": "medium",
  "provider": {
    "main": {
      "enabled": true,
      "name": "Main Provider",
      "base_url": "https://api.example.com/v1",
      "credential": "main_api_key",
      "wire_api": "openai_responses",
      "models": {
        "gpt-5.4": {
          "name": "GPT 5.4",
          "default_reasoning_effort": "medium"
        }
      }
    }
  }
}
```

`ProviderConfigFile` also accepts nested model capability metadata and arbitrary
custom provider/model records. The git-tracked built-in directory is
`crates/core/providers.json`; user and workspace JSON files are overlays.

The built-in directory covers Kimi, Z.ai and BigModel/Zhipu AI (each with
`glm-5.3` and `glm-5.3-flash`), DeepSeek,
Qwen, MiniMax, Xiaomi MiMo, Tencent Hunyuan, and a local Ollama template.
The Ollama entry uses `http://localhost:11434/v1` with an empty model list;
connected clients should refresh models through `provider/discover`
(`/v1/models`). This list is an overlayable catalog, not a restriction on
custom provider/model entries.

The canonical root fields are:

| Field | Type | Default | Meaning |
| --- | --- | --- | --- |
| `model` | string | first enabled model | Active `provider/model` reference. |
| `small_model` | string | automatic same-provider model, then `model` | Optional lower-cost model for lightweight background work. Invalid values use the same fallback. |
| `reasoning_effort` | string | model default | `default`, `off`, `on`, or a supported effort value. Legacy `disabled`/`enabled` normalize to `off`/`on`. |
| `provider` | object | `{}` | Provider id to provider record map. |

Provider records support `name`, `base_url`, `credential`, string-to-string
`headers`, `wire_api`, `enabled`, `env`,
`web_search`, `web_fetch`, and a `models` map. All are optional; `wire_api`
defaults to `openai_chat_completions` and `enabled` defaults to `true`.
`credential` refers to a credential id in the user-scoped `auth.json`; the
secret value is never stored in `providers.json`.

Model records support `name`, `wire_api`, `context_window`,
`effective_context_window_percent`, `max_tokens`, `temperature`, `top_p`,
`top_k`, `reasoning_capability`, `reasoning_implementation`,
`default_reasoning_effort`, `base_instructions`, `input_modalities`, `channel`,
`truncation_policy`, `supports_image_detail_original`, `enabled`, and
`priority`. They may also contain open-ended `family`, `release_date`, `status`,
`cost`, `metadata`, `options`, `request`, `headers`, `variants`, and
`default_variant` values. The nested model map key is both the provider-facing request id and
the `provider/model` identity; there is no separate `model_slug`, `model_name`,
`model_id`, or model `description` field.

`wire_api` has exactly three values:

| Value | Request family |
| --- | --- |
| `openai_chat_completions` | OpenAI-compatible Chat Completions |
| `openai_responses` | OpenAI-compatible Responses |
| `anthropic_messages` | Anthropic-compatible Messages |

`reasoning_capability` is one of `"unsupported"`, `"toggle"`, or
`{"levels":[...]}`. Include `off` in `levels` to allow disabling; omit `off`
if reasoning cannot be turned off. Legacy `{"toggle_with_levels":[...]}`
(and spelling `togglewithlevels`) still reads and migrates to `levels` with a
leading `off`. Toggle options are `off`/`on`. Effort values
are `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`.
`reasoning_implementation` is a legacy TOML-compatibility field. New JSON
should use `reasoning_capability` plus the named `variants` map (keys named
after logical selections; optional `request_model`) documented in
the configuration reference. The full
field reference, including JSON shapes for web capabilities and truncation,
is maintained in [`docs/configuration.md`](../../docs/configuration.md) and
its [Chinese version](../../docs/configuration.zh-Hans.md).

Onboarding keeps the provider template directory separate from Connection
model management. Selecting a saved Connection lists only its nested models;
the user can add a custom model or remove a saved model with d/Delete. Model
removal updates the user Connection overlay and never edits the tracked
provider directory.

Native `provider/discover` refreshes a connected Connection from its
`/models` or compatible `/v1/models` endpoint. It reads the credential from
the user-scoped `auth.json`, accepts OpenAI-style `data` arrays and provider
`models` arrays, normalizes common model metadata, and stores the raw entry in
the model's `metadata`. Discovery only updates the user Connection overlay;
the git-tracked `crates/core/providers.json` directory remains unchanged.

The old provider TOML shape remains readable only as a one-time startup
migration input; it is not the canonical write format and is never used to
build runtime provider settings after loading.

## Provider Credentials

`providers.json` contains the provider connection and a credential reference:

```json
{"provider":{"main":{"credential":"main_api_key"}}}
```

The actual secret is stored in the user-scoped `auth.json`:

```json
{
  "version": 1,
  "credentials": {
    "main_api_key": {
      "kind": "api_key",
      "value": "secret-value"
    }
  }
}
```

Do not put `apiKey` or `api_key` in `providers.json`. Reading an existing
`auth.json` fails if the schema version is unsupported or a credential value is
empty; a missing file is treated as an empty credential file.

## Web Search

`[tools.web_search]` controls whether a turn exposes web search to the model.
The effective value is resolved with this priority:

1. The selected model's `web_search` object in `providers.json`
2. The selected provider's `web_search` object in `providers.json`
3. `[tools.web_search]`

Supported modes:

- `disabled`: do not provide provider-hosted web search and do not expose the
  local `web_search` function tool.
- `provider`: let the active provider adapter inject provider-hosted search into
  the request. OpenAI Responses uses hosted tool `{"type":"web_search"}`;
  OpenAI Chat Completions uses `web_search_options`; Anthropic Messages uses
  server tool `{"type":"web_search_20250305","name":"web_search"}`.
- `local`: expose canonical function tool `web_search`, backed by a configured
  local provider under `[tools.web_search.local_providers.<id>]`.

Local providers currently support `kind = "exa"` and `kind = "tavily"`. Their
`credential` field is a credential id only; the secret value must live in
`<DEVO_HOME>/auth.json`. Optional `base_url` and `max_results` fields override
the provider default endpoint and result count. Compatibility aliases
`websearch` and `web-search` route to `web_search`, but aliases are not exposed
to the model.

## Web Fetch

`[tools.web_fetch]` controls whether a turn exposes URL fetching to the model.
It resolves with the same priority as web search:

1. The selected model's `web_fetch` object in `providers.json`
2. The selected provider's `web_fetch` object in `providers.json`
3. `[tools.web_fetch]`

Supported modes:

- `disabled`: do not provide provider-hosted web fetch and do not expose the
  local `webfetch` function tool.
- `provider`: let the active provider adapter inject provider-hosted fetch into
  the request. OpenAI Responses uses hosted tool `{"type":"web_fetch"}`;
  OpenAI Chat Completions uses `web_fetch_options`; Anthropic Messages uses
  server tool `{"type":"web_fetch_20250910","name":"web_fetch"}`.
- `local`: expose the existing local `webfetch` function tool. This is the
  default to preserve the existing local fetch behavior.

## Provider Resolution

The canonical resolver reads the standalone JSON file directly. It chooses the
active model in this order:

1. The top-level `model`, when it is a `provider/model` reference.
2. The first enabled model entry.

Runtime turn resolution uses an explicit canonical `provider/model` selection
and falls back to the first enabled directory model.

After a model is selected, resolution requires:

- The provider exists in `provider`.
- The provider is enabled.
- The model is enabled.
- The model's `wire_api`, or its provider's `wire_api`, is supported.
- If the provider references a credential, that credential exists in
  `auth.json`.

The resolved runtime settings contain the provider id, wire API, final model
name, optional base URL, optional API key, model limits, reasoning effort selection,
response-storage flag, and preferred auth method.

Model metadata starts from the tracked provider directory and is overlaid
field-by-field from user and workspace `providers.json` files. Repeating a
provider/model entry partially overrides it; a new nested model is a custom
model with safe defaults. The nested model key is also the provider-facing id
used for the API request, so there is no `model_slug`, `request_model`, or
`model_name` alias in new config.

A built-in partial override can be as small as:

```json
{"provider":{"deepseek":{"models":{"deepseek-v4-flash":{"context_window":262144,"effective_context_window_percent":90}}}}}
```

A custom model is selected directly through its provider/model reference:

```json
{
  "model": "example/custom",
  "provider": {
    "example": {
      "base_url": "https://api.example.com/v1",
      "credential": "example_api_key",
      "wire_api": "openai_responses",
      "models": {
        "custom": {
          "name": "Custom",
          "context_window": 128000,
          "reasoning_capability": {"levels": ["low", "medium", "high"]},
          "reasoning_implementation": "request_parameter",
          "default_reasoning_effort": "medium"
        }
      }
    }
  }
}
```

`ProviderModelConfig` exposes `name`, `channel`,
`context_window`, `effective_context_window_percent`, `max_tokens`, `temperature`,
`top_p`, `top_k`, `wire_api`, `reasoning_capability`,
`reasoning_implementation`, `default_reasoning_effort`, `base_instructions`,
`input_modalities`, `truncation_policy`, and `supports_image_detail_original`.
`name` is the picker label and `channel` groups related models. The effective context is
`context_window * effective_context_window_percent / 100` and is also the
automatic-compaction boundary; `max_tokens` is the default response-output
limit. `temperature`, `top_p`, and `top_k` are request sampling defaults.
`reasoning_capability` defines the choices shown to users, while
`reasoning_implementation` says whether a choice is disabled, sent as a request
parameter, or mapped to a configured wire-model variant;
`default_reasoning_effort` selects the initial effort. `input_modalities`
declares text/image input support, `truncation_policy` selects a byte- or
token-based request-content limit, and `supports_image_detail_original`
enables original-resolution image detail. Omitted built-in fields are
preserved. Omitted custom-model `base_instructions` use the default
instructions, while an explicit empty string means no base instructions.

The old TOML provider, binding, and model override fields remain readable as a
compatibility input. On startup, they are migrated to the matching
`providers.json` overlay before model resolution. User TOML moves to
`<DEVO_HOME>/providers.json`; workspace TOML moves to
`<workspace>/.devo/providers.json`. Existing JSON values win, API keys are
copied to user-scoped `auth.json`, and unrelated app settings stay in
`config.toml`. The migration is idempotent. The tracked
`crates/core/providers.json` file is the canonical built-in directory.

When reasoning effort resolution selects a model variant, the provider request
model is resolved within the selected provider namespace.

## Writing Provider Config

Provider writes use atomic file replacement. They write `providers.json` and
preserve unrelated application settings in `config.toml`.

`AppConfigStore::upsert_provider_connection` writes a provider Connection and
nested model record to the user-level `providers.json` file. The optional API
key argument is written to the user-scoped `auth.json`; only its credential id
is stored in `providers.json`. Project config may still override resolved
settings. Disconnecting a Connection removes its user overlay; built-in
provider templates are never modified.
