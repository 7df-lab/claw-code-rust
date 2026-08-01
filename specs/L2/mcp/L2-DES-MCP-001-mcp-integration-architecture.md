---
artifact_id: L2-DES-MCP-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-MCP-001 - MCP Integration Architecture

## Purpose

Define the technical design for integrating user-configured Model Context Protocol (MCP) servers into the program as discoverable, observable, and safety-gated capabilities.

## Background / Context

The MCP reference model separates three roles:

- The program is the MCP host.
- The program creates one MCP client per configured MCP server connection.
- External MCP servers provide capabilities such as tools, resources, resource templates, and prompts through standardized protocol operations.

MCP servers are useful because they let users connect the agent to external systems without hardcoding every integration. They are risky because the program does not own those external systems, their descriptions, or their side effects. The integration must therefore normalize MCP capabilities into the program's server-owned tool, context, configuration, and safety systems.

## Source Requirements

- `L1-REQ-APP-008` requires user-configured MCP integrations, discovery, status, startup errors, and safety participation.
- `L1-REQ-TOOL-001` requires tool safety, approval, redaction, and bounded output.
- `L1-REQ-TOOL-002` requires controlled tool execution through the built-in tool lifecycle.
- `L1-REQ-LLM-002` requires model-requested tool use to be validated and supervised.
- `L1-REQ-APP-010` requires persistent configuration and unavailable-state behavior.
- `L1-REQ-APP-011` requires actionable error recovery.
- `L1-REQ-APP-012` requires privacy, credential safety, and user data ownership.
- `L2-DES-APP-002` defines configuration precedence for user-scoped and workspace-scoped settings.
- `L2-DES-APP-003` defines client/server protocol events and request behavior.
- `L2-DES-AGENT-001` defines the execution engine that dispatches tools.
- `L2-DES-CONTEXT-001` defines context assembly.
- `L2-DES-TOOL-001` defines the server-owned tool registry and tool supervisor.

## Design Requirement

The program should integrate MCP by maintaining an MCP manager that owns configured server connections, lifecycle state, capability discovery, and normalized dispatch into the program's existing runtime boundaries.

MCP capabilities must not bypass the program's registry, context assembly, safety, approval, redaction, output bounding, observability, or durable recording. From the model's perspective, MCP-provided tools are available only as server-approved tool definitions. From the user's perspective, every MCP server has visible status and failures.

## Standards Alignment

The design should align with the MCP architecture and capability model:

| MCP concept | Program design |
|---|---|
| Host | The program runtime that owns sessions, context, tools, clients, and user control. |
| Client | A per-server connection object managed by the MCP manager. |
| Server | An external process or remote endpoint configured by the user or workspace. |
| Tools | Normalized into server-owned tool definitions and dispatched through the tool supervisor. |
| Resources | Exposed as readable context objects through a controlled resource-read path. |
| Resource templates | Exposed as typed resource patterns requiring parameter validation before read. |
| Prompts | Discovered as reusable prompt templates, but not automatically injected into model context. |
| Roots | Sent as advisory workspace context when policy allows; not treated as a security boundary. |
| Sampling | Disabled by default; if enabled later, host-owned model invocation, budgets, and approval apply. |
| Elicitation | Converted into user-visible pending input owned by the program, not raw server-to-model control. |
| Logging | Captured as server diagnostics with redaction and source attribution. |

The required L1 surface is tools, resources, resource templates, status, and safety. MCP prompts, sampling, and elicitation are protocol primitives that should be represented explicitly so the integration can fail safely or support them later without redefining the architecture.

## MCP Configuration

MCP configuration should follow the precedence and source-tracking rules from `L2-DES-APP-002`.

The concrete TOML shape for persisted MCP server records is defined by `L2-DES-APP-005` under `[mcp.servers.<server_id>]`. Secret material used by MCP servers is stored in companion `auth.json` files and referenced from TOML by credential id.

Conceptual `McpServerConfig` fields:

- `server_id`: stable local identifier.
- `display_name`: user-facing server name.
- `enabled`: whether the server may be used.
- `transport`: stdio, streamable HTTP, or another MCP-approved transport added later.
- `command`: command and arguments for stdio servers.
- `cwd`: optional working directory for stdio servers.
- `env`: non-secret environment values or credential-id references that the host injects into a stdio server process at runtime.
- `base_url`: endpoint for HTTP servers.
- `auth_ref`: `auth.json` credential id for HTTP authorization, not routine plaintext.
- `startup_policy`: eager, lazy, or manual.
- `trust_policy`: user, workspace, or untrusted workspace source.
- `allowed_capabilities`: optional allowlist for tools, resources, templates, prompts, sampling, and elicitation.
- `roots_policy`: which workspace roots may be shared with the server.
- `output_limits`: per-server output and diagnostic limits where configured.

Workspace-scoped MCP configuration can start local processes or route data to external services. Therefore workspace-scoped MCP servers must be visible to the user before first use. A workspace may suggest MCP servers, but the runtime should not silently grant broad trust to an unreviewed workspace configuration.

## Server Lifecycle

Each configured server has an independent lifecycle. One server failure must not disable built-in tools or unrelated MCP servers.

Conceptual server states:

- `disabled`
- `not_started`
- `starting`
- `ready`
- `degraded`
- `auth_required`
- `failed`
- `stopped`

Lifecycle rules:

- Eager servers start during runtime bootstrap if they are enabled and trusted for the current workspace.
- Lazy servers start when their status is inspected, a capability is needed, or the user requests refresh.
- Manual servers start only through explicit user action.
- Stdio server stdout is reserved for MCP protocol messages. Diagnostic output from stderr is captured as logs and bounded.
- HTTP server authentication failures produce `auth_required` or `failed` status with credential-safe diagnostics.
- Restart and refresh operations are per-server; they do not rebuild the whole runtime unless configuration precedence changes require it.

## Capability Discovery

After initialization and capability negotiation, the MCP manager discovers server catalogs.

Discovery should collect:

- Tool names, descriptions, annotations where available, and input schemas.
- Resource URIs, names, descriptions, MIME types where available, and size hints where available.
- Resource-template URI patterns, parameter descriptions, and descriptions.
- Prompt names, descriptions, and argument schemas where supported.
- Server capability flags and protocol version.
- Last successful refresh time and last failure.

Discovery output must be normalized into a catalog that retains source identity:

- `server_id`
- source configuration path and scope.
- original MCP name.
- normalized program-facing name.
- capability kind.
- user-facing description.
- schema or parameter contract.
- availability and failure state.

Descriptions supplied by an MCP server are user-facing hints, not trusted safety policy. The program must classify risk using configuration, schema, tool kind, permission policy, and runtime behavior.

## Tool Normalization

MCP tools should become ordinary tool definitions in the program registry.

Normalized MCP tool definition fields:

- `tool_name`: collision-free program-facing name.
- `display_name`: readable name including the originating server when needed.
- `server_id`
- `mcp_tool_name`: original MCP tool name.
- `description`
- `input_schema`
- `capability_kind`: mcp_tool.
- `execution_mode`: read_only, mutating, external_side_effect, network, or unknown.
- `permission_profile`
- `permission_policy`
- `redaction_policy`
- `output_limit_policy`
- `availability`

Name collisions must be impossible from the model-facing registry. The registry should namespace MCP tools by server identity or otherwise generate stable unique names while preserving the original MCP name in metadata.

## Tool Invocation

MCP tool calls follow the same lifecycle as built-in tool calls:

1. The model requests a normalized MCP tool.
2. The execution engine resolves the tool definition.
3. Input is validated against the MCP tool schema.
4. Mode, permission, safety, approval, configuration, and server availability gates run.
5. The MCP manager sends `tools/call` to the originating server only after gates pass.
6. Tool output is normalized into structured content, text content, resource references, and status.
7. Output is bounded and redacted before model, client, or durable exposure.
8. The tool result is recorded with server id, tool name, terminal state, and diagnostics.

If the originating server is unavailable, authentication is missing, or the tool disappeared after refresh, the call must complete with a structured unavailable result. It must not silently disappear from the transcript or be replaced with invented output.

## Resource Access

MCP resources should be treated as external context objects, not as automatic prompt content.

Rules:

- Resource lists may be shown to users and summarized to the model only when useful and bounded.
- Reading a resource requires an explicit resource-read operation selected by the user, a model-requested controlled tool, or another approved workflow.
- Resource reads must apply size limits, MIME-type filtering, redaction, and token-budget policy before context insertion.
- Resource templates require typed parameter validation before the URI is expanded.
- A resource URI is data, not authority. It must not bypass filesystem, network, or privacy policy.
- Resource subscriptions are optional. If supported, updates should produce client-visible resource-changed diagnostics and should not mutate prior transcript records.

Large resource catalogs should not be injected wholesale into every model context. The context assembler should prefer compact capability summaries and on-demand resource reads.

## Prompt Templates

MCP prompts are reusable server-provided prompt templates. They should be discovered and exposed as user-controllable templates, not automatically treated as higher-priority instructions.

Rules:

- Prompt discovery records names, descriptions, and argument schemas.
- Prompt execution, if enabled, should be a user-visible action that materializes prompt content into a normal submitted message or explicit context attachment.
- Prompt content must remain lower priority than system, developer, user, safety, and workspace instructions.
- Prompt output must not be injected silently as hidden model instructions.
- Prompt support may be deferred without blocking the required MCP tools/resources/resource-template integration, but unsupported prompts must be reported honestly in capability status.

## Roots, Sampling, And Elicitation

### Roots

When the runtime shares roots with an MCP server, roots are advisory context about the user's workspace. They are not a sandbox. The program must continue enforcing its own filesystem, command, permission, and approval checks for every tool call.

Workspace-scoped roots should be minimized to the active workspace and only shared with servers allowed by configuration and trust policy.

### Sampling

MCP sampling lets a server ask the host to perform an LLM call. Sampling is disabled by default.

If enabled by a later design, the host must own:

- Model selection.
- Prompt inspection and redaction.
- User approval where required.
- Token and cost budgets.
- Tool availability for the sampled call.
- Durable recording and attribution.

An MCP server must never receive an unrestricted pass-through to the user's configured model or secrets.

### Elicitation

MCP elicitation lets a server ask the user for structured input. Elicitation should be represented as a server-originated pending prompt owned by the program.

Rules:

- The prompt must identify the requesting server and operation.
- The user may approve, answer, deny, or cancel.
- Answers are returned only to the requesting MCP server for the active operation.
- Elicitation must not be used to bypass Plan Mode question-tool restrictions because it is not a model-facing question tool. It is an external-server prompt under user control.
- Secret collection through elicitation requires explicit credential-handling policy and must not be stored unless the user chooses a durable credential target.

## Context Assembly

MCP capability state feeds context assembly through tool availability and compact metadata, not through unbounded catalog injection.

Context assembly should include:

- Available normalized MCP tool schemas when they are enabled for the current session, mode, model, and permission posture.
- A compact MCP status summary only when relevant to the user request or recent failure.
- No raw resource content unless a resource was explicitly read and selected for context.
- No MCP prompt content unless the user or an approved workflow invoked the prompt.

When an MCP capability catalog changes during a session, the next context snapshot should reflect the new tool availability. If a model calls a stale tool, the runtime returns a structured unavailable result.

## Client Visibility

Clients should be able to show MCP integration state without reading raw configuration secrets.

Client projections should include:

- Server id and display name.
- Configuration source scope and safe source path where useful.
- Enabled/disabled state.
- Startup state.
- Authentication state.
- Last refreshed time.
- Safe startup or protocol error summary.
- Counts and names of tools, resources, resource templates, and prompts.
- Capability disabled reasons.

Representative protocol surfaces may include:

- `mcp/list`
- `mcp/tools`
- `mcp/set_enabled` (persist config and apply per-server start/stop live; rebuilds the tool registry Arc so the next turn sees catalog changes)
- `mcp.refreshServer` / `mcp.startServer` / `mcp.stopServer` (optional aliases later; `mcp/set_enabled` is the user-facing compose)
- `mcp.readResource`

These protocol surfaces are client/server methods. Model-requested MCP tool execution still flows through the tool supervisor.

Enable/disable must be host-owned: clients call `mcp/set_enabled` rather than writing `config.toml` and expecting a silent runtime apply. In-flight turns keep their existing registry Arc; subsequent turns use the swapped registry.

## Error Handling

MCP errors should be normalized into stable categories:

- Configuration invalid.
- Server disabled.
- Server not trusted.
- Startup failed.
- Transport failed.
- Protocol negotiation failed.
- Authentication required.
- Capability unavailable.
- Input invalid.
- Tool invocation failed.
- Resource read failed.
- Output rejected by policy.
- Operation canceled.

Every error shown to the user should include the affected server and safe recovery context. Errors must not print plaintext credentials or unbounded external output by default.

## Security And Privacy

MCP servers are external capability providers and must be treated as untrusted unless configured otherwise.

Security rules:

- Workspace-provided MCP servers require trust-aware visibility before first use.
- MCP tools use the same approval and safety model as built-in tools.
- MCP server-provided descriptions and schemas do not grant permission.
- Secrets are read from `auth.json` credential references and passed only to the specific configured server operation that requires them.
- Tool outputs, logs, and resources are redacted before model and client exposure.
- MCP servers cannot alter system, developer, or user instruction priority.
- MCP resources and prompts cannot silently become hidden instructions.
- Roots are advisory and must not replace host-side sandboxing or permission checks.
- Sampling is disabled unless a later approved design enables it with host-owned controls.

## Invariants

- The program is the MCP host and owns all user-facing state.
- Every configured MCP server has visible status.
- One MCP server failure does not disable unrelated capabilities.
- MCP tools execute only through the server-owned tool supervisor.
- MCP resource content enters model context only through explicit controlled read paths.
- MCP prompts are user-controlled templates, not automatic higher-priority instructions.
- MCP server credentials are not exposed in routine client projections.
- Stale or disappeared MCP capabilities fail with structured unavailable results.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-008 | 1 | specs/L1/L1-REQ-APP-008-mcp.md | Defines MCP configuration, lifecycle, capability discovery, status, safety, and failure behavior. |
| related-to | L1-REQ-TOOL-001 | 1 | specs/L1/L1-REQ-TOOL-001-safety.md | MCP tool calls follow the same safety, approval, redaction, and output limits as built-in tools. |
| related-to | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | MCP tools are normalized into the server-owned tool registry. |
| related-to | L1-REQ-LLM-002 | 1 | specs/L1/L1-REQ-LLM-002-tools.md | Model-requested MCP tool use is controlled by the execution engine and tool supervisor. |
| related-to | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | MCP servers are configured through user-scoped and workspace-scoped configuration. |
| related-to | L1-REQ-APP-011 | 1 | specs/L1/L1-REQ-APP-011-error-recovery.md | MCP startup, authentication, protocol, and tool errors require actionable recovery context. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | MCP credential and external-resource handling must preserve privacy boundaries. |
| related-to | L2-DES-APP-002 | 2 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Configuration precedence resolves MCP server records. |
| related-to | L2-DES-APP-005 | 2 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Defines concrete TOML fields for persisted MCP server records and `auth.json` credential references. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Clients inspect MCP state and receive MCP-related status events through the server protocol. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | The execution engine dispatches normalized MCP tool calls. |
| related-to | L2-DES-CONTEXT-001 | 1 | specs/L2/context/L2-DES-CONTEXT-001-context-assembly.md | MCP tool schemas and selected resources participate in context assembly. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | MCP tools are external tool definitions governed by the built-in tool lifecycle. |
| specified-by | L3-BEH-MCP-001 | 1 | specs/L3/mcp/L3-BEH-MCP-001-server-lifecycle-tool-normalization.md | L3 defines MCP lifecycle, capability discovery, tool normalization, resources, and error handling. |

## References

- [Model Context Protocol architecture](https://modelcontextprotocol.io/docs/learn/architecture)
- [Model Context Protocol server concepts](https://modelcontextprotocol.io/docs/learn/server-concepts)

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial MCP integration architecture based on MCP reference documentation and product requirements. |
| 1 | 2026-05-25 | Human | Refinement | Linked MCP configuration to the concrete `config.toml` schema. |
| 1 | 2026-05-25 | Human | Refinement | Clarified that MCP credentials are stored in companion `auth.json` files and injected only at runtime when needed. |
| 1 | 2026-05-25 | Human | Refinement | Renamed normalized MCP tool metadata from `approval_policy` to `permission_policy`. |
