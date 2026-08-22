# devo-protocol

This crate defines the protocol types shared by Devo clients and the Devo
server.

## ACP protocol

ACP support targets the stable v1.20 schema in `protocol-lock.json`. The schema
and v1 protocol documentation are normative for ACP wire behavior; the v2
draft is out of scope.

### Client-to-server methods

- `initialize`: negotiate protocol version, client capabilities, and server
  metadata.
- `authenticate`: authenticate when the server advertises an ACP authentication
  capability.
- `session/new`: create a new session for a working directory.
- `session/list`: list persisted sessions.
- `session/load`: load a persisted session and replay its conversation.
- `session/resume`: resume a persisted session without replaying its history.
- `session/close`: close an active session.
- `session/delete`: delete a persisted session.
- `session/prompt`: submit a prompt to an active session. The JSON-RPC response
  returns when the turn completes (`AcpPromptResult.stopReason`). Streaming
  progress is delivered through `session/update` notifications during the turn.
- `session/cancel`: cancel the active session turn. This is an ACP notification
  and has no JSON-RPC response.
- `session/set_mode`: select an advertised ACP session mode.
- `session/set_config_option`: update an advertised ACP session configuration
  option.
- `logout`: end the authenticated ACP client session when the server advertises
  the ACP logout capability.

### Server-to-client notifications

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

### Server-to-client requests

- `session/request_permission`: ask an ACP client to approve or reject a tool
  or runtime action.
- `fs/read_text_file`: ask an ACP client to read an absolute text-file path.
- `fs/write_text_file`: ask an ACP client to write text to an absolute file
  path.

### ACP behavior and compatibility

ACP `session/load` always replays the complete conversation before returning.
The desktop SDK applies display history limits locally after replay; it does
not send a Devo-specific history-limit extension.

ACP paths (`cwd`, additional directories, file-system paths, and tool-call
locations) are absolute. ACP stdio MCP commands are absolute executable paths.

ACP extensions must follow ACP's underscore-prefixed method/notification rule.
Devo metadata may be carried in `_meta`, but it must not add non-standard root
fields or change standard ACP replay semantics.

ACP clients use only standard ACP request/response methods advertised during
initialization. In particular, `userInput/request` is not an ACP extension.

## Native protocol

Native is Devo's first-party protocol surface. Devo-specific APIs belong here;
they are not ACP methods. Existing route modules may retain their historical
internal layout during migration, but new user-facing documentation should call
this surface Native.

On a shared JSON-RPC transport, Native is selected during `initialize` with
`_meta: { "devo": { "protocol": "native" } }`. Without that marker, the
connection remains on the ACP adapter surface.

Native clients use Native approval requests rather than ACP reverse requests.
Event-driven clients that need an immediate turn acknowledgement should use
Native `turn/start`, which returns a turn snapshot promptly and streams turn
progress through server notifications.

### Server-to-client requests

- `approval/command/request`, `approval/fileChange/request`, and
  `approval/permission/request`: request a decision for the corresponding
  pending approval.
- `userInput/request`: request structured user input.
- `session/goal/completionApproval/request`: request approval before completing
  a goal.

### Connection and subscription methods

- `initialize`: negotiate the Native connection and capabilities.
- `runtime/ping`: return the server time.
- `subscription/create`, `subscription/update`, `subscription/ack`, and
  `subscription/unsubscribe`: create, manage, acknowledge, and stop durable
  event subscriptions.

### Session methods

- `session/new`, `session/list`, `session/read`, and `session/resume`: create,
  list, read, and resume Native sessions.
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

### Turn methods

- `turn/start`: start a Devo turn with the Native turn request shape.
- `turn/steer`: send steering input directly to the active turn.
- `turn/read`: read a turn snapshot.
- `session/queue/steer`: send steering input into a running turn.

### Queue methods

- `session/queue/push`: add input to the session queue, starting it immediately
  when the session is available.
- `session/queue/list`, `session/queue/update`, and `session/queue/remove`:
  inspect, reorder or edit, and remove queued input.

### Workspace methods

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
- `context/usage/read`: read the context-window usage for a session.

### MCP methods

- `mcp/list`: list configured MCP servers.
- `mcp/tools`: list tools exposed by one MCP server.
- `mcp/set_enabled`: enable or disable one MCP server.

### Skills methods

- `skill/list`: list available skills for a working directory; pass
  `forceReload: true` after workspace changes.
- `skill/set_enabled`: persistently enable or disable a skill.

### Command execution methods

- `task/start` with `kind: "process"`: launch a command execution task.
- `task/read` and `task/list`: inspect one task or all tasks in a session.
- `task/write_stdin`, `task/resize`, and `task/interrupt`: control the
  session-owned process task returned by `task/start`.

### Goal methods

- `session/goal/set`: create or replace a session goal.
- `session/goal/read`: read the current goal state.
- `session/goal/update`: edit the current goal in place.
- `session/goal/pause`, `session/goal/resume`, `session/goal/complete`,
  `session/goal/cancel`, and `session/goal/clear`: transition or clear a goal.

### Message methods

- `session/message/edit`: replace a user message and supersede the affected
  turn.

### Agent methods

- `task/start` with `kind: "agent"`: spawn a subagent task.
- `agent/list` and `agent/read`: inspect subagent tasks.
- `agent/message`: send a follow-up message to a subagent.
- `agent/cancel`: stop a subagent task.

### Reference search methods

- `search/start`: start a server-backed composer reference search.
- `search/update`: update the active reference-search query.
- `search/cancel`: cancel the active reference search.

### Current implementation exceptions

`NATIVE_METHODS` in `src/native/methods.rs` is the Native API contract and the
source for generated schemas and client bindings. The server runtime is not yet
fully aligned with that contract:

- Registered but not routed by the server: `session/cwd/change`,
  `session/archive`, `turn/steer`, `turn/read`, `tool/list`,
  `permission/profile/read`, `permission/profile/update`, `credential/list`,
  `credential/set`, and `credential/delete`. Clients cannot currently call
  these methods successfully despite their Native registration.
- Routed by the server but absent from `NATIVE_METHODS`: `task/start` and
  `task/resize`. They work at runtime, but are omitted from generated Native
  schemas and client bindings.
- Transitional routes outside the Native registry: `session/start` and
  `command/exec`. New clients should use the registered Native surface rather
  than depending on these routes.
