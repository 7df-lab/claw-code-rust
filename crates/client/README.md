# devo-client

`devo-client` contains client transports for talking to the Devo runtime server.
It exposes a stdio client that spawns a server process, and a WebSocket client
that connects to an already-running server. Both transports send JSON-RPC
request/notification messages and read responses/events from the same
connection.

## Public Interfaces

- `StdioServerClientConfig`: spawn configuration for the stdio server process,
  including the program path, extra arguments, and ACP client capabilities to
  advertise during initialization.
- `ServerNotificationMessage`: raw server notification with a method name and
  JSON params.
- `StdioServerClient`: async stdio transport client. It owns the child process,
  request routing, notification stream, and shutdown path.
- `WebSocketServerClientConfig`: WebSocket endpoint and ACP client
  capabilities for an existing server listener.
- `WebSocketServerClient`: async WebSocket transport client. It sends one
  JSON-RPC message per text frame and multiplexes responses, notifications, and
  ACP server-to-client requests on the same socket.

Start a WebSocket-only server with:

```sh
devo server --transport websocket
```

Configure explicit listeners with `server.listen = ["ws://127.0.0.1:3210"]`.
The short `ws://` listen target uses `127.0.0.1:3210`.

## Client Methods

- `spawn`: start the server process and attach stdin/stdout/stderr readers.
- `connect`: connect to an existing WebSocket server.
- `initialize`: perform the ACP protocol handshake.
- `session_start`, `session_resume`, `session_list`: create, resume, and list
  sessions.
- `session_settings_update`, `session_model_update`,
  `session_title_update_native`: update session metadata through the
  native patch API.
- `session_compact_start_native`, `session_fork_native`, and the
  native rollback preview/commit methods manage session history and
  derived sessions.
- `agent_list_native`, `agent_read_native`, `agent_message_native`,
  and `agent_cancel_native` inspect and manage background agents.
- `session_goal_set_native`: create or replace a session goal through the
  native session API.
- `skill_list_native`, `skill_set_enabled_native`: read and update skill
  catalog state through the native wire shapes.
- Native `model/list` and `model/preferences/*` are available through the
  typed native client methods.
- `provider_list`, `provider_upsert`, `provider_validate`: manage provider
  configuration through the native wire shapes.
- `mcp_list`, `mcp_tools`, `mcp_set_enabled`: inspect and update MCP servers
  through the native wire shapes.
- `command_exec`: launch the remaining sessionless command-execution path.
  Session-owned process control uses the native `task/*` methods.
- `turn_start`, `session_interrupt`, `turn_steer`: drive and interrupt active work.
- `approval_respond`, `request_user_input_respond`: answer pending server
  prompts.
- `search_start`, `search_update`, `search_cancel`: control native,
  connection-local reference search workflows.
- `recv_notification`: receive the next raw server notification.
- `recv_event`: receive and decode the next notification as a `ServerEvent`.
- ACP server-to-client requests: handles permission prompts. Client filesystem
  requests are not supported by Devo clients.
- `shutdown`: close the transport and release associated client resources.
