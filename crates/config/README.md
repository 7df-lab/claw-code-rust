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

The user-level config file is `<DEVO_HOME>/config.toml`. `DEVO_HOME` defaults to
`~/.devo`; if the environment variable is set, it must point to an existing
directory.

When a workspace is known, the project-level config file is:

```text
<workspace>/.devo/config.toml
```

Provider credentials are stored separately in:

```text
<DEVO_HOME>/auth.json
```

`auth.json` stores secret values, while `config.toml` stores references to those
credentials.

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

Provider config is part of `config.toml` and is modeled by `ProviderConfigSection`.
The current provider schema uses provider vendor entries plus model bindings:

```toml
model = "gpt-5.4"
model_reasoning_effort_selection = "medium"
model_auto_compact_token_limit = 970000
model_context_window = 997500
disable_response_storage = true
preferred_auth_method = "apikey"

[defaults]
model_binding = "gpt54-main"

[providers.main]
enabled = true
name = "Main Provider"
base_url = "https://api.example.com/v1"
credential = "main_api_key"
wire_apis = ["openai_responses"]

[model_bindings.gpt54-main]
enabled = true
model_slug = "gpt-5.4"
provider = "main"
request_model = "gpt-5.4"
invocation_method = "openai_responses"
default_reasoning_effort = "medium"
```

Supported `wire_apis` and `invocation_method` values are:

- `openai_chat_completions`
- `openai_responses`
- `anthropic_messages`

The default `invocation_method` is `openai_chat_completions` when a model
binding omits it.

`preferred_auth_method` accepts `apikey` and `api_key`; it serializes as
`apikey`.

Legacy `[model_providers]` fields still deserialize into `ProviderConfigSection`,
but the provider resolver does not use legacy-only config to produce runtime
provider settings.

## Provider Credentials

`auth.json` is modeled by `UserAuthConfigFile`. Example
`<DEVO_HOME>/auth.json`:

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

The credential id, such as `main_api_key`, is referenced from provider config
with `credential = "main_api_key"`.

Only `api_key` credentials are currently supported. Reading `auth.json` fails if
the schema version is unsupported or a credential value is empty. Missing
`auth.json` is treated as an empty credential file.

## Web Search

`[tools.web_search]` controls whether a turn exposes web search to the model.
The effective value is resolved with this priority:

1. `[model_bindings.<id>.web_search]`
2. `[providers.<id>.web_search]`
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

1. `[model_bindings.<id>.web_fetch]`
2. `[providers.<id>.web_fetch]`
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

`resolve_provider_settings_from_config_and_auth` chooses the active model
binding in this order:

1. `[defaults].model_binding`, when it points to an existing binding.
2. The top-level `model`, when it matches a binding's `model_slug` or
   `request_model`.
3. The first enabled model binding.

Runtime turn resolution uses an explicit requested model first, when it matches
an enabled binding's `model_slug` or `request_model`. Without a requested model, it
uses `[defaults].model_binding` only when that binding is enabled, then falls
back to the first enabled binding.

After a binding is selected, resolution requires:

- The binding's `provider` exists in `[providers]`.
- The provider is enabled.
- The binding is enabled.
- The binding's `model_slug` exists in the effective model catalog.
- If the provider lists `wire_apis`, the binding's `invocation_method` is in
  that list.
- If the provider references a credential, that credential exists in
  `auth.json`.

The resolved runtime settings contain the provider id, wire API, final model
name, optional base URL, optional API key, model limits, reasoning effort selection,
response-storage flag, and preferred auth method.

`model_slug` is the local catalog key. Model metadata starts from the built-in
catalog and is overlaid field-by-field from user and workspace `config.toml`
`[model.<slug>]` sections. Existing slugs are partial overrides; new slugs create
custom models with safe defaults. `request_model` is the provider-facing model id
used for the API request. The legacy `model_name` key is accepted when reading
existing configuration, while subsequent writes use `request_model`. Turn
metadata records `model` as the catalog slug and `request_model` as the provider
request model; these values may be identical.

Model metadata `provider` describes the wire API and accepts
`openai_chat_completions`, `openai_responses`, or `anthropic_messages`. The
binding's `invocation_method` is the operational connection choice and should
match that metadata. A usable custom model therefore needs a
`[providers.<id>]` connection and `[model_bindings.<id>]` binding; the provider's
optional `credential` points to an API key stored in `auth.json`.

A built-in partial override can be as small as:

```toml
[model.qwen3-coder-next]
context_window = 262144
effective_context_window_percent = 90
```

A custom model must also be selected through connection wiring:

```toml
[defaults]
model_binding = "custom-example"

[model.custom]
display_name = "Custom"
provider = "openai_responses"
context_window = 128000
reasoning_capability = { levels = ["low", "medium", "high"] }
reasoning_implementation = "request_parameter"
default_reasoning_effort = "medium"

[providers.example]
enabled = true
name = "Example"
base_url = "https://api.example.com/v1"
credential = "example_api_key"
wire_apis = ["openai_responses"]

[model_bindings.custom-example]
enabled = true
model_slug = "custom"
provider = "example"
request_model = "provider-facing-model-id"
invocation_method = "openai_responses"
```

`ModelOverrideConfig` exposes `display_name`, `description`, `channel`,
`context_window`, `effective_context_window_percent`, `max_tokens`, `temperature`,
`top_p`, `top_k`, `provider`, `reasoning_capability`,
`reasoning_implementation`, `default_reasoning_effort`, `base_instructions`,
`input_modalities`, `truncation_policy`, and `supports_image_detail_original`.
`display_name` is the picker label, `description` is its explanatory text, and
`channel` groups related models. The effective context is
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

Old `<DEVO_HOME>/models.json` and `<workspace>/.devo/models.json` files are
ignored. Migration is manual: copy desired fields into the corresponding user or
workspace `config.toml` `[model.<slug>]` sections. The legacy top-level scalar
`model = "slug"` remains readable, but it collides with the new `model` table
namespace, so new configuration must select with `[defaults].model_binding`.

When reasoning effort resolution selects a model variant catalog slug, the provider
request model is resolved from enabled bindings for the same provider as the
selected turn binding. Duplicate `model_slug` values under other providers do
not affect that request.

## Writing Provider Config

Provider writes use atomic file replacement. They preserve unrelated TOML in
`config.toml` and only overlay provider-owned keys.

`AppConfigStore::upsert_provider_vendor` writes provider vendors and model
bindings to the user config. Project config may still override resolved
settings, but onboarding and provider management persist shared provider setup in
the user-level `config.toml`. The upsert rejects provider vendors with an empty
`wire_apis` list and reloads the effective app config after a successful write.
