# devo-protocol

This crate defines the protocol types shared by Devo clients and the Devo
server.

## ACP v1.20 and Devo Native extensions

ACP support targets the stable v1.20 schema in `protocol-lock.json`. The schema
and v1 protocol documentation are normative for ACP wire behavior; the v2
draft is out of scope.

The current client-to-server ACP methods are:

- `initialize`: negotiate protocol version, client capabilities, and server
  metadata.
- `session/new`: create a new session for a working directory.
- `session/list`: list persisted sessions.
- `session/resume`: resume a persisted session without replaying its history.
- `session/prompt`: submit a prompt to an active session. The JSON-RPC response
  returns when the turn completes (`AcpPromptResult.stopReason`). Streaming
  progress is delivered through `session/update` notifications during the turn.
- `session/cancel`: cancel the active session turn. This is an ACP notification
  and has no JSON-RPC response.
- `logout`: end the authenticated ACP client session when the server advertises
  the ACP logout capability.

ACP `session/load` always replays the complete conversation before returning.
The desktop SDK applies display history limits locally after replay; it does
not send a Devo-specific history-limit extension.

ACP paths (`cwd`, additional directories, file-system paths, and tool-call
locations) are absolute. ACP stdio MCP commands are absolute executable paths.

Event-driven clients that need an immediate turn acknowledgement should use
Native `turn/start`, which returns a turn snapshot promptly and streams turn
progress through server notifications.

The current server-to-client ACP notification method is:

- `session/update`: stream session lifecycle, item, plan, usage, and turn-status
  updates to subscribed clients. The payload is an `AcpSessionNotification`
  whose `update.sessionUpdate` discriminator can include:
  - `session_info_update`: session title and update timestamp changes.
  - `user_message_chunk`: streamed user message content.
  - `agent_message_chunk`: streamed assistant message content.
  - `agent_thought_chunk`: streamed assistant reasoning or reasoning-summary
    content.
  - `tool_call`: initial tool or command-execution call metadata, including
    tool call id, title, kind, status, raw input, content, and locations.
  - `tool_call_update`: status, output, content, terminal, diff, or location
    updates for an existing tool call.
  - `plan`: current plan entries and their statuses.
  - `available_commands_update`: slash commands currently available to the
    client, including command descriptions and optional input hints.
  - `current_mode_update`: the current ACP session mode id.
  - `config_option_update`: configurable ACP session options currently exposed
    by the server.
  - `usage_update`: context-window usage and optional cost information.

The current server-to-client ACP request methods are:

- `session/request_permission`: ask an ACP client to approve or reject a tool
  or runtime action.
- `fs/read_text_file`: ask an ACP client to read an absolute text-file path.
- `fs/write_text_file`: ask an ACP client to write text to an absolute file
  path.

First-party Native clients do not implement these ACP reverse requests;
Native approval requests are used for those clients instead.

Devo-specific client-to-server APIs belong to the Native surface. Existing
route modules may retain their historical internal layout during migration,
but users and new documentation should call this surface Native. They are not
ACP methods.
In particular, `userInput/request` is a Native server-initiated request and is
not registered as an ACP extension.

ACP extensions must follow ACP's underscore-prefixed method/notification rule.
Devo metadata may be carried in `_meta`, but it must not add non-standard root
fields or change standard ACP replay semantics.

### Session extensions

- `session/metadata/update`: update session metadata and settings with the
  Native patch shape, including title, model, reasoning effort, permission
  preset, sandbox profile, and compaction threshold.
- `session/compact/start`: start a manual compaction turn; keep emitting
  `session/compaction/*` for UI.
- `session/fork`: fork a new session from an existing turn.
- `session/rollback/preview` followed by `session/rollback/commit`: roll back
  a session to a selected user turn with an explicit restore plan.
- `session/interrupt`: stop the active session turn, a Native task, or a
  sessionless command process through one scoped request.

### Turn extensions

- `turn/start`: start a Devo turn with the Native turn request shape.
- `session/queue/steer`: send steering input into a running turn.

### Workspace extensions

- `workspace/changes/read`: read branch, uncommitted, or turn-scoped
  workspace change views. Git workspaces support branch and uncommitted scopes;
  non-Git workspaces report those scopes as unsupported and only expose
  turn-scoped bounded filesystem snapshots.
- `workspace/changes/updated`: notify subscribed clients that the turn-scoped
  workspace change summary was finalized or updated. The notification carries a
  summary only; clients call `workspace/changes/read` for full diffs.

### Provider and model methods

- `provider/list`: list configured providers using the Native camelCase
  result.
- `provider/upsert`: add or update a provider and optional model binding.
- `provider/validate`: validate provider credentials and model settings.
- `model/list` and `model/preferences/*`: read and update the Native model
  catalog and preferences.

### MCP methods

- `mcp/list`: list configured MCP servers.
- `mcp/tools`: list tools exposed by one MCP server.
- `mcp/set_enabled`: enable or disable one MCP server.

### Skills methods

- `skill/list`: list available skills for a working directory; pass
  `forceReload: true` after workspace changes.
- `skill/set_enabled`: persistently enable or disable a skill.

### Command execution extensions

- `task/start` with `kind: "process"`: launch a command execution task.
- `task/write_stdin`, `task/resize`, and `task/interrupt`: control the
  session-owned process task returned by `task/start`.

### Goal methods

- `session/goal/set`: create or replace a session goal.
- `session/goal/read`: read the current goal state.
- `session/goal/update`: edit the current goal in place.
- `session/goal/pause`, `session/goal/resume`, `session/goal/complete`,
  `session/goal/cancel`, and `session/goal/clear`: transition or clear a goal.

### Agent extensions

- `task/start` with `kind: "agent"`: spawn a subagent task.
- `agent/list` and `agent/read`: inspect subagent tasks.
- `agent/message`: send a follow-up message to a subagent.
- `agent/cancel`: stop a subagent task.

### Reference search and user-input methods

- `search/start`: start a server-backed composer reference search.
- `search/update`: update the active reference-search query.
- `search/cancel`: cancel the active reference search.
Native-only user-input requests are deliberately excluded from the ACP method
registry. ACP clients use only the standard ACP request/response methods
advertised during initialization.
