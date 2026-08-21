---
artifact_id: L2-DES-APP-005
revision: 3
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-19
---

# L2-DES-APP-005 - Config TOML And Auth JSON Schema

## Purpose

Define the durable `config.toml` file shape used by user-scoped and workspace-scoped configuration, and the user-scoped `auth.json` file shape used for credentials.

This design makes model/provider persistence, `/model` selection defaults, MCP server setup, skill discovery roots, and routine application preferences inspectable and mergeable without requiring clients to understand implementation-specific runtime state. It also keeps API keys and other credential material out of `config.toml`.

## Background / Context

`L2-DES-APP-002` defines where configuration files live and how workspace-scoped configuration overlays user-scoped configuration field by field. This document defines the TOML schema stored at those locations.

The same schema is used for:

- User-scoped configuration:
  - Windows: `C:\Users\username\.devo\config.toml`
  - Windows auth file: `C:\Users\username\.devo\auth.json`
  - macOS and Linux: `~/.devo/config.toml`
  - macOS and Linux auth file: `~/.devo/auth.json`
- Workspace-scoped configuration:
  - `<workspace>/.devo/config.toml`

The TOML schema stores durable preferences and durable setup records. It must not store transient session state, resolved runtime profiles, provider request payloads, routine client projections, API keys, or other secret values.

The JSON auth schema stores credential material only in the user configuration directory. `config.toml` files refer to auth entries by stable credential id.

## Source Requirements

- `L1-REQ-APP-010` requires persistent configuration, user/project file locations, project-over-user precedence, effective configuration inspection, and deterministic persistence targets.
- `L1-REQ-MODEL-001` requires persisted invocable model configuration.
- `L1-REQ-MODEL-002` requires persisted provider configuration and safe credential handling.
- `L1-REQ-MODEL-003` requires onboarding-created configuration to be restorable.
- `L1-REQ-TUI-010` requires onboarding to submit setup results for persistence.
- `L1-REQ-APP-008` requires MCP integrations to be user-configured and status-visible.
- `L1-REQ-APP-009` requires skills to be discoverable from configured sources with visible missing/unavailable states.
- `L1-REQ-APP-003` requires permission modes, sandboxing, user approval for out-of-boundary actions, and fail-closed behavior when permission state is ambiguous.
- `L1-REQ-APP-012` requires credential and privacy-safe projections.

## Design Requirement

Configuration should use a single TOML schema version with stable, keyed records. Credential material should use a user-scoped `auth.json` schema version with stable credential ids.

Keyed TOML tables are preferred for mergeable records because workspace-scoped configuration can override fields, disable records, or add records by stable identifier:

- `[providers.<provider_id>]`
- `[model.<slug>]`
- `[model_bindings.<binding_id>]`
- `[mcp_servers.<server_id>]`
- `[skills.roots.<root_id>]`

Arrays may be used only for ordered scalar settings where record-level override is not needed.

The file should be readable and editable by users, but it is still a program-owned schema. The program should validate before persisting changes and should avoid inventing placeholder values when validation fails.

`config.toml` is the durable configuration file. `auth.json` is the durable credential file. Environment variables and external secret stores are not the designed persistence mechanism for API keys or other credentials.

## Top-Level Shape

Every config file should declare the schema version:

```toml
schema_version = 1
```

Top-level sections:

```text
[defaults]
[provider_http]
[providers.<provider_id>]
[model.<slug>]
[model_bindings.<binding_id>]
[tools.web_search]
[mcp]
[mcp_servers.<server_id>]
[skills]
[skills.roots.<root_id>]
[experimental]
[workspace.instructions]
[tui]
[telemetry]
[logging]
```

Sections may be absent. Missing sections mean no values are supplied by that source.

Unknown top-level sections should be preserved on write when possible. Unknown keys under known sections should produce diagnostics unless they are under a documented extension namespace such as `[x.<namespace>]`.

## Complete Example

```toml
schema_version = 1

[defaults]
model_binding = "gpt55-openrouter"
reasoning_effort = "medium"
mode = "default"
theme = "system"
permission_policy = "default"

[provider_http]
proxy_url = "http://proxy.corp.com:8080"

[providers.openrouter]
enabled = true
name = "OpenRouter"
base_url = "https://openrouter.ai/api/v1"
credential = "openrouter_api_key"
headers = '{"X-Title":"Devo","Authorization":"custom-token"}'

[providers.openai]
enabled = true
name = "OpenAI"
base_url = "https://api.openai.com/v1"
credential = "openai_api_key"

[model."openai/gpt-5.5"]
effective_context_window_percent = 90

[model_bindings.gpt55-openrouter]
enabled = true
model_slug = "openai/gpt-5.5"
display_name = "GPT 5.5"
provider = "openrouter"
request_model = "openai/gpt-5.5"
invocation_method = "openai_responses"
default_reasoning_effort = "medium"

[model_bindings.deepseek-v4-pro]
enabled = true
model_slug = "deepseek/deepseek-v4-pro"
display_name = "DeepSeek V4 Pro"
provider = "openrouter"
request_model = "deepseek/deepseek-v4-pro"
invocation_method = "openai_chat_completions"
default_reasoning_effort = "high"

[tools.web_search]
mode = "provider"

[providers.openai.web_search]
mode = "provider"

[tools.web_search.local_providers.exa]
kind = "exa"
credential = "exa_api_key"
max_results = 5

[mcp]
auto_start = true

[mcp_servers.github]
command = "github-mcp-server"
args = ["stdio"]
startup_policy = "lazy"
trust_policy = "user"
allowed_capabilities = ["tools", "resources"]
roots_policy = "workspace"

[mcp_servers.github.env]
GITHUB_TOKEN = { credential = "github_token" }

[skills]
enabled = true
model_catalog_enabled = true
auto_activate = false

[skills.roots.user]
enabled = true
path = "~/.devo/skills"
trust_policy = "user"

[skills.roots.interop]
enabled = true
path = "~/.agents/skills"
trust_policy = "user"

[workspace.instructions]
fallback_filenames = ["CLAUDE.md", "PROMPT.md"]
max_bytes = 200000

[tui]
theme = "system"
vim_mode = false

[telemetry]
enabled = false

[logging]
level = "info"
```

Companion user-scoped `auth.json` example:

```json
{
  "schema_version": 1,
  "credentials": {
    "openrouter_api_key": {
      "kind": "api_key",
      "value": "sk-or-example",
      "created_at": "2026-05-25T00:00:00Z",
      "updated_at": "2026-05-25T00:00:00Z"
    },
    "openai_api_key": {
      "kind": "api_key",
      "value": "sk-example",
      "created_at": "2026-05-25T00:00:00Z",
      "updated_at": "2026-05-25T00:00:00Z"
    },
    "github_token": {
      "kind": "bearer_token",
      "value": "ghp_example",
      "created_at": "2026-05-25T00:00:00Z",
      "updated_at": "2026-05-25T00:00:00Z"
    }
  }
}
```

## Defaults

`[defaults]` stores durable default selections and user preferences.

Fields:

- `model_binding`: optional binding id from `[model_bindings]`.
- `reasoning_effort`: optional logical reasoning effort string for the default model selection.
- `mode`: optional default collaboration mode id.
- `theme`: optional UI theme id.
- `permission_policy`: optional default permission posture.

Top-level preference keys (outside `[defaults]`) that the TUI/runtime also honor:

- `theme`: optional UI theme id (also accepted historically under `[defaults]`).
- `collapse_reasoning`: optional boolean controlling reasoning display collapsing.
- `compaction_token_limit`: optional absolute auto-compaction token threshold applied
  globally to all sessions (clamped per model `context_window`).

Rules:

- `model_binding` must reference an enabled effective model binding.
- `reasoning_effort` must be allowed by the selected binding's supported model definition.
- Defaults are not active-session state. After the first user message, changing the active session model or reasoning effort does not rewrite `[defaults]` unless a workflow explicitly persists a default according to `L2-DES-APP-002`.
- If both a binding-level `default_reasoning_effort` and `[defaults].reasoning_effort` are present, `[defaults].reasoning_effort` is the default session selection for `[defaults].model_binding`. The binding-level value remains the default shown when that binding is selected outside the current default.

## Permission Policy

`[defaults].permission_policy` controls the default tool permission posture.

Allowed values:

- `default`: baseline permission behavior. Read-only inspection may proceed where otherwise allowed; mutating file operations, command execution, network access, external side effects, and privileged operations remain subject to normal validation, permission checks, sandbox checks, and user approval when required.
- `auto_review`: review-oriented permission behavior. The runtime should classify tool calls before execution and prefer automatic review/diagnostic feedback for risky or ambiguous operations. User approval is still required when the review cannot prove the action is within policy.
- `full_access`: broad permission behavior for trusted contexts. The runtime minimizes approval prompts for allowed tool calls, but it must still enforce validation, mode constraints, privacy rules, audit recording, and the configured sandbox.

Rules:

- The field name is `permission_policy`.
- `approve_policy` and `approval_policy` are not schema fields. If encountered, they should produce a migration or validation diagnostic rather than silently changing behavior.
- Permission policy does not define filesystem or network isolation. Sandbox policy is a separate execution restriction layer.
- `full_access` does not mean unrestricted host execution when sandbox restrictions are configured.

## Sandbox Direction

The durable sandbox schema is not finalized in this revision. The earlier placeholder value `sandbox = "workspace-write"` is intentionally not part of the current `config.toml` schema.

The sandbox design target is to restrict the system calls available to tool execution processes, especially calls that open, read, write, create, delete, rename, or otherwise mutate filesystem objects.

The sandbox should eventually control:

- Directory read access.
- Directory write access.
- File creation, mutation, rename, and deletion.
- Process execution boundaries where supported.
- Network access, controlled at the domain level.

Conceptual future policy dimensions:

- Filesystem read roots and denied roots.
- Filesystem write roots and denied roots.
- Whether tool execution may spawn child processes.
- Whether network access is disabled, unrestricted, or restricted to allowed domains.
- Domain allowlists and denylists for network-capable tools and processes.

Sandbox policy is enforced by the host around tool execution. It is not a model-visible promise and not a substitute for permission policy, user approval, or tool validation.

## Provider HTTP

`[provider_http]` stores HTTP transport settings shared by model-provider requests.

Optional fields:

- `proxy_url`: proxy URL used by model-provider HTTP requests.

Rules:

- `proxy_url` applies to OpenAI Chat Completions, OpenAI Responses, Anthropic Messages, and provider validation requests.
- `proxy_url` must not change MCP server requests, web fetch or web search tool requests, update checks, or unrelated HTTP clients.
- A workspace-scoped `proxy_url` replaces the user-scoped `proxy_url` when both are present.

## Providers

`[providers.<provider_id>]` stores reusable user-defined provider endpoints.

Required fields for an enabled provider:

- `enabled`: boolean.
- `name`: user-facing provider display name.
- `base_url`: provider API base URL.
- `credential`: credential id from effective `auth.json`.

Optional fields:

- `availability_status`: last known safe status such as `unknown`, `valid`, `auth_required`, or `unavailable`.
- `timeout_ms`: provider request timeout.
- `connect_timeout_ms`: provider connection timeout.
- `headers`: JSON object encoded as a TOML string. Object keys are HTTP header names and object values must be strings.

Provider ids are stable program-generated identifiers. Changing `name` must not change the provider id.

Provider records must not contain model-specific fields such as `request_model`, legacy `model_name`, `model_slug`, binding `display_name`, `invocation_method`, or reasoning effort.

Custom provider headers:

- `headers` is parsed only as a JSON object string, for example `headers = '{"X-Foo":"bar"}'`.
- Header names and values must be valid HTTP header names and values.
- Header values may contain secret material because this is an explicit raw configuration path.
- Custom headers are applied after provider-owned headers. If a custom header duplicates a provider-owned header such as `Authorization`, `Content-Type`, `x-api-key`, or `anthropic-version`, the custom header value wins.
- Routine client projections, logs, transcripts, and telemetry must not expose plaintext custom header values by default.

## Credentials

`config.toml` stores credential references. `auth.json` stores credential material.

Provider records reference credentials by id:

```toml
credential = "openrouter_api_key"
```

MCP process environment entries and HTTP auth entries also reference `auth.json` credentials by id:

```toml
[mcp_servers.github.env]
GITHUB_TOKEN = { credential = "github_token" }

[mcp_servers.linear]
url = "https://mcp.linear.app"
auth_ref = "linear_api_key"
```

`auth.json` has the following shape:

```json
{
  "schema_version": 1,
  "credentials": {
    "openrouter_api_key": {
      "kind": "api_key",
      "value": "sk-or-example"
    }
  }
}
```

Rules:

- `auth.json` is the only designed durable storage location for API keys and other credential material.
- Environment variables, OS keychains, external credential stores, and inline TOML secrets are not part of this design except for explicit raw custom provider HTTP header values.
- Routine client projections must show credential status, not plaintext credential values.
- `config.toml` writes must not insert plaintext credential values except when the user explicitly configures raw custom provider HTTP header values.
- `auth.json` writes must update only the intended credential entries.
- Errors may name the provider, MCP server, credential id, and invalid custom header name, but must not print credential or custom header values by default.
- Workspace-scoped `auth.json` is not part of the durable credential design. Credential material must remain in user-scoped `auth.json`.
- Workspace-scoped `config.toml` may reference credential ids from user-scoped `auth.json`, and may define explicit raw custom provider HTTP header values, but it must not otherwise store plaintext credential material.

Auth records:

- `kind`: `api_key`, `bearer_token`, or another program-known credential kind.
- `value`: secret value.
- `created_at`: optional timestamp.
- `updated_at`: optional timestamp.
- `description`: optional user-facing label.

Credential ids are stable keys inside `auth.json`. Renaming a provider does not rename its credential id.

## Model Metadata

`[model.<slug>]` stores partial metadata overrides for embedded models and
metadata for custom model slugs. The embedded catalog is the only model catalog
base. Effective metadata is produced by overlaying user-scoped and then
workspace-scoped `[model.<slug>]` fields; an omitted field preserves the
lower-priority or embedded value.

Supported fields include `display_name`, `description`, `channel`,
`context_window`, `effective_context_window_percent`, `max_tokens`,
`temperature`, `top_p`, `top_k`, `provider`, `reasoning_capability`,
`reasoning_implementation`, `default_reasoning_effort`, `base_instructions`,
`input_modalities`, `truncation_policy`, and
`supports_image_detail_original`. A new slug creates a custom model definition
with safe defaults; invocability still requires a matching provider and model
binding.

Legacy top-level `model = "slug"` remains readable, but new configuration must
select through `[defaults].model_binding` because `[model.<slug>]` owns the
top-level `model` table namespace. Old user `~/.devo/models.json` and workspace
`<workspace>/.devo/models.json` files are ignored. Migration is manual: copy
desired metadata fields into the corresponding user or workspace
`config.toml` `[model.<slug>]` section, then retain or add the required provider
and binding records. Credential values remain in user-scoped `auth.json`.

## Model Bindings

`[model_bindings.<binding_id>]` stores configured invocable models.

Required fields for an enabled binding:

- `enabled`: boolean.
- `model_slug`: canonical model slug matching the effective model catalog.
- `display_name`: user-configurable client display label for this binding.
- `provider`: provider id from `[providers]`.
- `request_model`: provider-specific model identifier used for API requests.
- `invocation_method`: program-known invocation method id.

Optional fields:

- `default_reasoning_effort`: logical reasoning effort selected by onboarding or default setup.
- `availability_status`: last known safe status.

Legacy configuration may provide `model_name` as a read alias for `request_model`. Writes persist only `request_model` and remove `model_name` from updated bindings.

Allowed `invocation_method` values for the initial schema:

- `openai_responses`
- `openai_chat_completions`
- `anthropic_messages`

Rules:

- `model_slug` must exist in the effective model catalog produced from embedded definitions plus the field-wise merged user and workspace `[model.<slug>]` configuration.
- `display_name` is display metadata only. It must not be used as a stable identifier, provider API model name, or cross-reference key.
- Program-created model bindings must persist `display_name`. When the user accepts the default suggestion, that persisted value should be copied from the built-in supported model definition's display name.
- `provider` must reference an enabled effective provider.
- `invocation_method` must be supported by the program and valid for the provider/model combination.
- `default_reasoning_effort` must be absent when the supported model does not allow reasoning.
- `default_reasoning_effort` must be one of the supported model's logical effort values when present.
- The `/model` command's first list is populated from enabled effective model bindings. It may show the binding's model and provider together, but it must ask for reasoning effort as a separate step when the model supports reasoning.

## MCP Host Settings

`[mcp]` stores host-level MCP settings that are not per-server:

- `auto_start`: boolean (default `true`). Whether enabled servers should be auto-started during bootstrap.

## MCP Servers

`[mcp_servers.<server_id>]` stores configured MCP server connections. The TOML table key is the stable server id.

Transport is inferred from which fields are present — there is no `transport` or `kind` discriminator field:

- `command` present, `url` absent → stdio transport.
- `url` present, `command` absent → HTTP transport. The optional `type` field selects `http` (alias `streamable_http`, the default) or the deprecated `sse`.
- Both `command` and `url` present, or neither present, is a validation error.

### Stdio fields

- `command` (string, required): executable name or path.
- `args` (string array, optional): arguments passed to the command.
- `cwd` (string, optional): working directory for the child process.
- `env` (string map, optional): literal environment variables for the child process. Values that reference `auth.json` use `{ credential = "credential_id" }`.
- `env_vars` (string array, optional): environment variable names forwarded from the host process.

### HTTP / SSE fields

- `url` (string, required): MCP server endpoint URL.
- `type` (string, optional): `http`, `streamable_http`, or `sse`. Defaults to `http` (streamable HTTP) when omitted.
- `http_headers` (string map, optional): literal HTTP headers sent to the server.
- `env_http_headers` (string map, optional): header name → environment variable name. The header value is read from the named host environment variable at connection time.
- `auth_ref` (string, optional): `auth.json` credential id for HTTP authorization. Preferred over plaintext tokens.

### Common optional fields (omit on write when default)

- `enabled` (boolean, default `true`).
- `display_name` (string, default = server id).
- `startup_policy`: `eager`, `lazy`, or `manual` (default `lazy`).
- `trust_policy`: `user`, `workspace`, or `untrusted` (default `user`).
- `allowed_capabilities` (string array, optional): allowlist containing `tools`, `resources`, `resource_templates`, `prompts`, `sampling`, or `elicitation`. Omit for unrestricted.
- `roots_policy`: `none`, `workspace`, or a custom string array (default `none`).
- `output_limits.max_tool_output_bytes` (integer, optional, default 1 MiB).
- `output_limits.max_resource_bytes` (integer, optional, default 10 MiB).

Writes must not emit fields or sub-tables whose values equal the defaults. Empty maps (`env`, `http_headers`, `env_http_headers`), empty arrays (`args`, `allowed_capabilities`, `env_vars`), and default `output_limits` must be omitted.

### Examples

Stdio server:

```toml
[mcp_servers.radare2]
command = "/usr/bin/r2pm"
args = ["-r", "r2mcp"]
```

Stdio server with environment credential:

```toml
[mcp_servers.github]
command = "github-mcp-server"
args = ["stdio"]
startup_policy = "lazy"
allowed_capabilities = ["tools", "resources"]
roots_policy = "workspace"

[mcp_servers.github.env]
GITHUB_TOKEN = { credential = "github_token" }
```

Streamable HTTP with literal bearer token header:

```toml
[mcp_servers.linear]
url = "https://mcp.linear.app"

[mcp_servers.linear.http_headers]
Authorization = "Bearer token_value"
```

Deprecated SSE transport:

```toml
[mcp_servers.ida_mcp]
url = "http://127.0.0.1:13337/sse"
type = "sse"
```

### Rules

- Enabled stdio servers require `command`.
- Enabled HTTP/SSE servers require `url`.
- Secret-bearing process environment variables and HTTP auth values must reference `auth.json` credential ids or host environment variables. Plaintext tokens must not be written into `config.toml`.
- The runtime may inject an `auth.json` credential into a child process environment only for the configured server operation that requires it.
- Workspace-scoped MCP servers must be visible to the user before first use because they may start local processes or send workspace data to external services.
- `[mcp_servers.<server_id>]` follows the keyed record merge semantics: workspace overlays user field-by-field by server id.

## Skills

`[skills]` controls global skill behavior for the configuration source.

Fields:

- `enabled`: whether skill discovery is enabled.
- `model_catalog_enabled`: whether a concise skill catalog may be offered to the model.
- `auto_activate`: whether model-selected skill activation is allowed without an explicit user naming a skill.

`[skills.roots.<root_id>]` stores skill discovery roots.

Fields:

- `enabled`: boolean.
- `path`: directory path.
- `trust_policy`: `user`, `workspace`, `plugin`, or `untrusted`.
- `max_depth`: optional scan depth for package discovery.

Rules:

- Skill roots are discovery roots, not instructions by themselves.
- Supporting files under a skill root are not loaded during configuration load.
- Workspace skill roots should be trust-visible before automatic activation.
- Duplicate skill names must be resolved deterministically or reported as conflicts by the skill catalog, not silently overwritten by configuration merge.

## Experimental

`[experimental]` is reserved for runtime feature gates.

The former `code-search` boolean gate has been removed. Semantic code search is
provided by the bundled MCP server `code_search` (`devo-code-search-mcp`), which
is present in MCP config with `enabled = false` by default. Users enable it via
`devo mcp enable code_search` or the TUI `/mcps` UI.

Legacy `[experimental] code-search` / `code_search` keys are ignored for
backward compatibility.

## Bundled MCP servers

Devo ensures the following server is present after config load when missing by
id (user records with the same id are never overwritten):

```toml
[mcp_servers.code_search]
command = "devo-code-search-mcp"
enabled = false
display_name = "Code Search"
startup_policy = "lazy"
```

## Tools

Tool configuration should be grouped by tool family.

Web search shape:

```toml
[tools.web_search]
mode = "local" # disabled, provider, or local
local_provider = "exa"

[tools.web_search.local_providers.exa]
kind = "exa" # exa or tavily
credential = "exa_api_key"
max_results = 5

[providers.openai.web_search]
mode = "provider"

[model_bindings.gpt55-openrouter.web_search]
mode = "disabled"
```

Rules:

- Effective mode priority is model binding > provider > global `[tools.web_search]`.
- `mode = "disabled"` means the runtime does not provide any web search tool to the model.
- `mode = "provider"` uses the active provider adapter's hosted search shape.
- `mode = "local"` exposes canonical function tool `web_search` and calls the configured Exa or Tavily local provider.
- Local provider `credential` values are references to user-scoped `auth.json`; API keys must not be written into project config.
- Only one web search path may be active for a turn.

## Workspace Instructions

`[workspace.instructions]` stores project-instruction discovery preferences.

Fields:

- `fallback_filenames`: ordered list of additional instruction filenames after native priority files.
- `max_bytes`: maximum assembled instruction bytes before truncation.

Rules:

- The native instruction priority remains owned by `L2-DES-WORKSPACE-001`.
- Additional filenames are configuration-driven compatibility fallbacks.
- Truncation must be visible to the user or diagnostic projection.

## Record Merge Semantics

The effective configuration is built from user config, then workspace config.

For scalar settings:

- Workspace value replaces user value when the same scalar field is present.
- Missing workspace value leaves user value in effect.

For keyed record collections:

- Record identity is the TOML table key.
- A workspace record with the same key merges with the user record field by field.
- A workspace field with the same field name replaces the user field.
- User fields not mentioned by the workspace record remain in effect.
- A workspace record may disable a user record by setting only `enabled = false`.
- Workspace records with new keys are added to the effective set.
- User records with keys not mentioned by workspace config remain in the effective set.

Every effective leaf field must retain provenance so inspection can explain mixed user/workspace records.

Credential references resolve against user-scoped auth data:

- User `auth.json` provides the complete credential set.
- Workspace config may reference user-scoped credential ids.
- The workspace directory must not contain a durable `auth.json`.
- Invalid or missing credential references produce actionable errors and must not silently fall back to another credential id.

## Validation

The program should validate configuration in two phases:

1. Parse and schema validation for each source independently.
2. Effective configuration validation after precedence is applied.

Source validation catches malformed TOML, unsupported schema versions, wrong value types, missing required fields in enabled records, and unknown keys that cannot be preserved safely.

Auth validation catches malformed JSON, unsupported auth schema versions, wrong value types, missing credential values, duplicate credential ids inside the user auth file, and credential kinds unsupported by the referring config field.

Effective validation catches references to missing providers, disabled providers, missing model bindings, missing credentials, invalid supported model slugs, invalid model display names, invalid invocation methods, unsupported reasoning efforts, invalid provider HTTP proxy URLs, invalid provider custom header JSON, invalid provider custom header names or values, invalid MCP transport combinations, and unavailable skill roots.

Invalid higher-priority configuration must produce an actionable error instead of silently falling back to lower-priority values for the same setting.

## Write Safety

Configuration writes should be schema-aware:

- Preserve unrelated sections and unknown extension sections where possible.
- Update only the target section or record.
- Never write placeholder model names, provider ids, invocation methods, or reasoning values after validation failure.
- Validate the full resulting file before replacing the previous file contents.
- Use an atomic write strategy in L3/implementation design.
- Report the target path and setting being changed without exposing plaintext credentials or custom header values by default.

Auth writes should be schema-aware:

- Update `auth.json`, not `config.toml`, when only a credential value changes.
- Never log or show the prior or new credential value by default.
- Validate the resulting `auth.json` before replacing the previous file contents.
- Use an atomic write strategy in L3/implementation design.

Workflows that create or modify providers, model bindings, MCP servers, skill roots, or credentials are persistence writes. Selecting an already-configured model binding for a running session is not a provider, binding, or credential rewrite.

When a setup flow writes both `config.toml` and `auth.json`, the program should avoid committing a final `config.toml` that references a credential id absent from the final `auth.json`. Exact two-file commit and recovery mechanics belong in L3 design.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Defines the concrete `config.toml` and `auth.json` schemas for persistent configuration. |
| related-to | L1-REQ-MODEL-001 | 1 | specs/L1/L1-REQ-MODEL-001-config.md | Model bindings and defaults are persisted in `config.toml`, while credentials are referenced through `auth.json`. |
| related-to | L1-REQ-MODEL-002 | 1 | specs/L1/L1-REQ-MODEL-002-provider.md | User-defined providers are persisted in `config.toml`, with credential material in `auth.json`. |
| related-to | L1-REQ-MODEL-003 | 1 | specs/L1/L1-REQ-MODEL-003-onboard.md | Onboarding writes provider and model binding records into `config.toml` and credentials into `auth.json`. |
| related-to | L1-REQ-TUI-010 | 1 | specs/L1/L1-REQ-TUI-010-onboarding-ui.md | TUI onboarding collects values persisted by this schema. |
| related-to | L1-REQ-APP-008 | 1 | specs/L1/L1-REQ-APP-008-mcp.md | MCP servers are configured through `config.toml` and use `auth.json` credential references. |
| related-to | L1-REQ-APP-009 | 1 | specs/L1/L1-REQ-APP-009-skills.md | Skill roots and enablement are configured through `config.toml`. |
| related-to | L1-REQ-APP-003 | 1 | specs/L1/L1-REQ-APP-003-safety.md | `[defaults].permission_policy` persists the default permission posture without replacing runtime approval or sandbox enforcement. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | `auth.json` credential storage and redaction behavior protect secrets. |
| related-to | L2-DES-APP-002 | 2 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Source precedence resolves this schema across user and workspace files. |
| related-to | L2-DES-MODEL-001 | 2 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | Provides concrete persistence fields for providers and model bindings. |
| related-to | L2-DES-MCP-001 | 1 | specs/L2/mcp/L2-DES-MCP-001-mcp-integration-architecture.md | Provides concrete persistence fields for MCP servers. |
| related-to | L2-DES-SKILLS-001 | 1 | specs/L2/skills/L2-DES-SKILLS-001-agent-skills-architecture.md | Provides concrete persistence fields for skill roots and enablement. |
| specified-by | L3-BEH-APP-001 | 3 | specs/L3/app/L3-BEH-APP-001-configuration-resolution.md | Defines schema validation, field-level merge behavior, safe inspection, and atomic per-file write behavior for `config.toml` and user-scoped `auth.json`. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial TOML schema design for durable user and project configuration. |
| 1 | 2026-05-25 | Human | Refinement | Moved API keys and other credential material into dedicated `auth.json` files and removed environment variables or external stores as the recommended credential persistence mechanism. |
| 2 | 2026-05-27 | Human | Refinement | Moved workspace config to `<workspace>/.devo/config.toml`, made `auth.json` user-scoped only, and changed record precedence to field-level merge. |
| 1 | 2026-05-25 | Assistant | Refinement | Linked persisted `permission_policy` defaults to application safety requirements. |
| 1 | 2026-05-26 | Human | Refinement | Added explicit model binding `display_name` examples and clarified display-name fallback and identifier rules. |
| 2 | 2026-06-08 | Human | Refinement | Added global provider HTTP proxy configuration and per-provider raw custom header configuration. |
| 3 | 2026-08-19 | Human | Schema change | Replaced `[[mcp.servers]]` array-of-tables and nested `[mcp.servers.transport]` with keyed `[mcp_servers.<server_id>]` tables. Transport is inferred from fields (`command` → stdio, `url` → HTTP/SSE). Writes omit default-valued fields. Merge is field-by-field by server id. No backward-compatible reader for the old array shape. |
