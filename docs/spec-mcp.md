# MCP Detailed Specification

## Background and Goals

`devo` must support external MCP servers as a first-class capability source, not as an ad hoc transport add-on. MCP integration must allow the runtime to discover tools, resources, and resource templates from external servers while preserving the same safety, persistence, and event guarantees used for built-in tools.

Primary goals:

- define crate and module boundaries for MCP integration
- support configured MCP server discovery, startup, refresh, and status tracking
- normalize MCP tools and resources into `devo` runtime contracts
- preserve safety, approval, and observability across MCP calls

## Scope

In scope:

- configured MCP server lifecycle
- MCP tool and resource discovery
- MCP request dispatch and response normalization
- auth-state reporting and refresh hooks
- MCP elicitation and approval integration

Out of scope:

- inventing a new protocol instead of MCP
- marketplace UX for distributing MCP servers

## Module Responsibilities and Boundaries

`devo-mcp` owns:

- reading normalized MCP server config
- starting and stopping configured MCP clients
- server capability discovery
- tool/resource/resource-template catalog refresh
- auth and startup status tracking
- request dispatch to MCP servers
- MCP-specific error normalization

`devo-tools` owns:

- wrapping discovered MCP tools into model-visible tool definitions when enabled

`devo-server` owns:

- API methods and events for MCP status, reload, and elicitation routing

`devo-safety` owns:

- approval and policy enforcement around MCP tool execution when needed

## Core Data Structures

```rust
pub struct McpServerId(pub SmolStr);

pub struct McpServerRecord {
    pub id: McpServerId,
    pub display_name: String,
    pub transport: McpTransportConfig,
    pub startup_policy: McpStartupPolicy,
    pub enabled: bool,
}
```

```rust
pub enum McpTransportConfig {
    Stdio {
        command: Vec<String>,
        cwd: Option<PathBuf>,
        env: BTreeMap<String, String>,
    },
    StreamableHttp {
        base_url: String,
        auth: Option<McpAuthConfig>,
    },
}
```

```rust
pub struct McpServerStatus {
    pub server_id: McpServerId,
    pub startup_state: McpStartupState,
    pub auth_state: McpAuthState,
    pub tools: Vec<McpToolDescriptor>,
    pub resources: Vec<McpResourceDescriptor>,
    pub resource_templates: Vec<McpResourceTemplateDescriptor>,
}
```

## Lifecycle

1. load configured MCP server records from app config
2. start enabled servers according to startup policy
3. fetch server capabilities and tool/resource catalogs
4. expose eligible MCP tools to the runtime tool registry
5. route tool and resource requests through `devo-mcp`
6. surface startup/auth changes through server events
7. allow explicit reload without restarting the whole runtime

Rules:

- MCP startup failures must not crash the whole runtime
- per-server failure isolation is required
- loaded session execution may continue even if some MCP servers are unavailable

## Tool and Resource Integration

Rules:

- MCP tools must be normalized into the same high-level runtime lifecycle as built-in tools
- MCP resources and templates must remain explicit non-tool capabilities
- model-visible exposure of MCP tools must be controlled by runtime config and safety policy
- MCP tool calls that require user input or approvals must route through the same server-initiated request system as built-in approvals

## Elicitation and Approval

Rules:

- MCP server elicitation must be surfaced as a server-initiated request to the client
- elicitation payloads must include the originating server id and correlated `sessionId` or `turnId` when available
- resolution must emit the same request-resolution cleanup event pattern as other server requests

## Configuration Definitions

```rust
pub struct McpConfig {
    pub servers: Vec<McpServerRecord>,
    pub auto_start: bool,
    pub refresh_on_config_reload: bool,
}
```

## Error Handling Strategy

Required error categories:

- `McpServerUnavailable`
- `McpStartupFailed`
- `McpAuthRequired`
- `McpProtocolError`
- `McpToolInvocationFailed`
- `McpResourceReadFailed`

Rules:

- MCP errors must be attributed to the originating server
- auth failures must be distinguishable from transport failures
- stale catalogs must be refreshable without restarting the process

## Observability

Required logs and metrics:

- `mcp.server.start`
- `mcp.server.ready`
- `mcp.server.failed`
- `mcp.server.auth_required`
- `mcp.tool.call.count`
- `mcp.tool.call.failure.count`
- `mcp.catalog.refresh.duration_ms`

## Testing Strategy and Acceptance Criteria

Required tests:

- configured server startup and failure isolation
- tool catalog refresh
- MCP elicitation routing
- auth-state change propagation
- MCP tool normalization into runtime tool definitions

Acceptance criteria:

- enabled MCP servers can contribute tools and resources without bypassing safety or persistence rules
- MCP server failure degrades only that server, not the whole runtime
