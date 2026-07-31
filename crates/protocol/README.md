# devo-protocol

This crate defines the protocol types shared by Devo clients and the Devo
server.

## ACP and Devo extension methods

Devo uses ACP JSON-RPC methods for the portable protocol surface. The current
client-to-server ACP methods are:

- `initialize`: negotiate protocol version, client capabilities, and server
  metadata.
- `session/new`: create a new session for a working directory.
- `session/list`: list persisted sessions.
- `session/resume`: load a persisted session.
- `session/prompt`: submit a prompt to an active session. The JSON-RPC response
  returns when the turn completes (`AcpPromptResult.stopReason`). Streaming
  progress is delivered through `session/update` notifications during the turn.
- `session/cancel`: cancel the active session turn.

Event-driven clients that need an immediate turn acknowledgement should use the
Devo extension `_devo/turn/start`, which returns `TurnStartResult::Started`
promptly and streams turn progress through server notifications.

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

- `session/request_permission`: ask the client to approve or reject a tool or
  runtime action.
- `fs/read_text_file`: ask the client to read an absolute text-file path.
- `fs/write_text_file`: ask the client to write text to an absolute file path.

Devo-specific client-to-server APIs are sent with the `_devo/` method prefix.
The prefix is applied by the client transport, then removed by the server before
dispatching to `ClientMethod`. These methods remain non-standard ACP extension
points because they expose Devo-specific TUI, runtime, or local workflow
behavior that is not represented by the portable ACP method set.

### Session extensions

- `_devo/session/title/update`: rename a session from the client.
- `_devo/session/metadata/update`: update session metadata such as the active
  model or reasoning-effort selection.
- `_devo/session/permissions/update`: update the current permission preset.
- `_devo/session/compact`: proactively compact a session context.
- `_devo/session/fork`: fork a new session from an existing turn.
- `_devo/session/rollback`: roll back a session to a selected user turn.

### Turn extensions

- `_devo/turn/start`: start a Devo turn with the full Devo turn request shape.
  If an older server does not support it, the client falls back to ACP
  `session/prompt`.
- `_devo/turn/shell_command`: run a user shell command through the server
  runtime.
- `_devo/turn/interrupt`: interrupt the active Devo turn.
- `_devo/turn/steer`: send steering input into a running turn.

### Workspace extensions

- `_devo/workspace/changes/read`: read branch, uncommitted, or turn-scoped
  workspace change views. Git workspaces support branch and uncommitted scopes;
  non-Git workspaces report those scopes as unsupported and only expose
  turn-scoped bounded filesystem snapshots.
- `workspace/changes/updated`: notify subscribed clients that the turn-scoped
  workspace change summary was finalized or updated. The notification carries a
  summary only; clients call `_devo/workspace/changes/read` for full diffs.

### Provider and model extensions

- `_devo/provider/list`: list configured provider vendors.
- `_devo/provider/upsert`: add or update a provider vendor and optional model
  binding.
- `_devo/provider/validate`: validate provider credentials and model settings.
- `_devo/model/catalog`: read the effective model catalog.
- `_devo/model/saved`: notify the server that model configuration was saved.

### Skills extensions

- `_devo/skills/list`: list available skills for a working directory.
- `_devo/skills/changed`: notify the server that skill files changed.
- `_devo/skills/set_enabled`: persistently enable or disable a skill.

### Command execution extensions

- `_devo/command/exec`: launch a command execution request.
- `_devo/command/exec/write`: write input to a running command.
- `_devo/command/exec/resize`: resize a running command terminal.
- `_devo/command/exec/terminate`: terminate a running command.

### Goal extensions

- `_devo/goal/create`: create a goal for the active thread.
- `_devo/goal/set`: update the current goal objective.
- `_devo/goal/status`: read the current goal state.
- `_devo/goal/pause`: pause goal continuation.
- `_devo/goal/resume`: resume goal continuation.
- `_devo/goal/complete`: mark the goal complete.
- `_devo/goal/clear`: clear the current goal.

### Agent extensions

- `_devo/agent/list`: list subagents associated with a session.
- `_devo/agent/spawn`: spawn a subagent.
- `_devo/agent/close`: close a subagent.

### Reference search and user-input extensions

- `_devo/search/start`: start a server-backed composer reference search.
- `_devo/search/update`: update the active reference-search query.
- `_devo/search/cancel`: cancel the active reference search.
- `_devo/request_user_input/respond`: answer a pending structured user-input
  request.
