---
artifact_id: L2-DES-APP-003
revision: 2
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-APP-003 — Client Server Protocol

## Purpose

Refine the client/server architecture requirement into a protocol, transport, and process-ownership design that supports TUI, desktop, IDE, and future clients sharing the same agent runtime.

## Background / Context

The program has multiple potential client surfaces. Each client needs to start or resume sessions, submit turns, observe streaming output, answer approval or question prompts, and inspect shared state.

If every client launches its own private server process over stdio, those clients cannot naturally share the same active sessions or runtime state. Shared state requires a discoverable server instance that multiple clients can connect to concurrently.

## Source Requirements

- `L1-REQ-APP-001` requires client surfaces to share server-side agent behavior.
- `L1-REQ-CONV-001` requires durable session lifecycle behavior.
- `L1-REQ-CONV-002` requires observable turn lifecycle behavior.
- `L1-REQ-CONV-003` requires explicit `steer` and `queue` handling during active turns.
- `L1-REQ-CONV-004` requires session forking from a specific turn and fork traceability.
- `L1-REQ-CONV-005` requires editing the immediately preceding eligible user-authored message without mutating durable history.
- `L1-REQ-APP-002` requires persistence and recovery behavior.
- `L1-REQ-APP-003` requires permission modes, sandboxing, explicit approval for out-of-boundary actions, and user-visible approval outcomes.
- `L1-REQ-AGENT-001` requires a complete execution workflow with visible task state.
- `L1-REQ-AGENT-002` requires interrupt, cancel, inspect, and resume behavior.
- `L1-REQ-AGENT-003` requires visible task planning with status updates.
- `L1-REQ-CHANGE-001` requires rollback and recovery behavior for file changes.
- `L1-REQ-EDIT-001` requires file edits to be reviewable and recoverable.
- `L1-REQ-GOAL-001` requires user-owned Ralph Loop goal controls and visible bounded autonomous continuation state.
- `L1-REQ-GIT-001` constrains git-oriented change management.
- `L1-REQ-APP-010` requires effective configuration inspection and model/reasoning updates.
- `L1-REQ-APP-011` requires actionable error recovery and provider error detail presentation.
- `L1-REQ-APP-012` requires user-data ownership, export, deletion, and credential-safe projections.
- `L1-REQ-AGENT-005` restricts the question tool to Plan Mode.
- `L1-REQ-TUI-008` requires leading `!` input to execute terminal commands through the program.
- `L1-REQ-TUI-009` requires TUI Build, Plan, and Shell input modes.
- `L1-REQ-APP-006` requires fuzzy search across user-facing entities.
- `L1-REQ-CLIENT-004` requires `@` prefixed input to open fuzzy search.
- `L1-REQ-TOOL-001` requires tool output safety and redaction visibility.
- `L1-REQ-TOOL-002` requires baseline built-in tools, including planning, approval, questions, search, command execution, web, and delegation tools.
- `L1-REQ-MEM-001` defines persistent memory as core-maintained internal state outside the routine client-server protocol surface.
- `L2-DES-TOOL-001` defines the built-in tool system and plan tool.
- `L2-DES-GOAL-001` defines Ralph Loop goal state, continuation, and budget behavior.
- `L2-DES-APP-006` defines fuzzy-search providers and project file search.
- `L2-DES-CLIENT-002` defines the `@` prefixed input action that consumes fuzzy search.
- `L2-DES-CONV-001` defines durable session JSONL events and distinguishes provider, server-client, and durable event planes.

## Protocol Requirement

The program should use JSON-RPC 2.0 as the logical client/server protocol envelope.

JSON-RPC is suitable because it supports:

- Request/response calls for commands that need results.
- Notifications for one-way events.
- Transport-independent message semantics.
- Reuse over WebSocket while preserving a simple method and notification model.

The protocol should define program-specific method names and event payloads rather than exposing provider-specific SSE events directly to clients.

## Transports

The program should use JSON-RPC 2.0 over WebSocket as the client/server transport.

The local server should bind a loopback WebSocket endpoint by default. TUI, desktop, IDE, and browser-capable clients should all connect to that endpoint as WebSocket clients.

WebSocket is the required transport because it supports concurrent local clients and also fits browser-extension and desktop-client constraints. `stdio` should not be the shared-client transport because a stdio child process is normally owned by one parent client and cannot naturally be discovered and shared by TUI, desktop, and IDE clients at the same time.

## Server Instance Ownership

The default local architecture should use a single discoverable server instance per user profile.

Conceptual startup flow:

```text
Client starts
        ↓
Read server endpoint descriptor
        ↓
Try to connect to existing server
        ↓
If unavailable, acquire startup lock
        ↓
Start detached server
        ↓
Write endpoint descriptor
        ↓
Connect and authenticate
```

The endpoint descriptor should be stored in a user-scoped runtime location and include:

- Server process identifier where available.
- WebSocket endpoint URL.
- Authentication token or credential reference.
- Server version.
- Started-at timestamp.

The descriptor must be protected by user-only filesystem permissions where the operating system supports them.

## Request Response Contract

Every client request must receive a JSON-RPC response. The response confirms whether the server accepted the command, rejected it, or completed a read-only query.

For long-running operations, the response must be immediate and must not wait for the full turn, tool call, or model invocation to finish. The response allocates canonical identifiers and sequence positions; subsequent progress is delivered through server notifications.

Successful responses should include:

- `accepted`: whether the command was accepted for execution.
- Canonical identifiers created or resolved by the server, such as `session_id`, `turn_id`, `item_id`, `subscription_id`, `approval_id`, or `question_id`.
- `latest_sequence` or `next_sequence` where ordering or catch-up matters.
- A projection or snapshot when the request is a read operation.
- A safe message or warning when the request succeeds with degraded behavior.
- An idempotency result when a repeated request uses a previously seen client-generated id.

Rejected responses should use JSON-RPC error responses with:

- `code`: stable machine-readable error code.
- `message`: concise user-facing explanation.
- `data`: structured recovery context, such as missing permission, invalid session, stale sequence, invalid model, or unavailable provider.

## JSON-RPC Method Naming

JSON-RPC request and notification method names must use `/` path separators rather than `.` separators.

Rules:

- Use names such as `server/initialize`, `turn/start`, and `config/update`.
- Do not introduce dot-form aliases such as `server.initialize`, `turn.submit`, or `config.update`.
- Existing notification names from `crates/protocol/src/event.rs::ServerEvent::method_name()` should be reused where the semantic event already exists. Examples include `session/started`, `turn/started`, `turn/usage/updated`, `item/started`, `item/agentMessage/delta`, `inputQueue/updated`, `steer/accepted`, and `serverRequest/resolved`.
- Conceptual event-kind names such as `turn_started` or `item_content_update` may still be used inside projections or explanatory tables, but the JSON-RPC envelope `method` string uses the slash-separated method name.

## Client Requests

Representative client-to-server JSON-RPC request methods and response results:

| Method | Purpose | Important Params | Server Response Result |
|---|---|---|---|
| `server/initialize` | Register a client connection, authenticate it, and negotiate protocol compatibility. | `client_id`, `client_kind`, `protocol_version`, `auth_token`, `client_capabilities`, `workspace_root` where known. | `server_id`, `server_version`, `protocol_version`, `server_capabilities`, `latest_sequence`. |
| `server/shutdown` | Request a graceful server shutdown when the caller is authorized to do so. | `reason`, `client_id`, optional `force_after_timeout`. | `accepted`, `shutdown_state`, optional `message`. |
| `session/list` | Return session summaries for pickers, recent-session views, and restore flows. | `workspace_filter`, `include_archived`, `limit`, `cursor`, optional sort order. | `sessions`, `next_cursor`, `latest_sequence`. |
| `session/open` | Load a session and return a current projection without necessarily subscribing to future events. | `session_id`, `projection`, optional `from_sequence`. | `session_snapshot`, `latest_sequence`. |
| `session/create` | Create a new session record when the client explicitly starts a new session before submitting a turn. | `workspace_root`, `initial_metadata`, optional `client_generated_label`. | `session_id`, `session_snapshot`, `latest_sequence`. |
| `session/fork` | Create a child session that inherits visible history from a parent session without deep-copying all parent records. | `parent_session_id`, `fork_turn_id`, `workspace_root`, optional `fork_label`. | `session_id`, `parent_session_id`, `fork_turn_id`, `inherited_segment_id`, `session_snapshot`. |
| `session/archive` | Mark a session as archived so it leaves active-session views while remaining recoverable where policy allows. | `session_id`, `archive_reason`. | `session_id`, `archived`, `latest_sequence`. |
| `session/export` | Export session history and allowed related data for user data portability. | `session_id`, `include_inherited_history`, `redaction_level`, `format`. | `export_id`, `accepted`, `status`, optional `download_ref`, `latest_sequence`. |
| `session/subscribe` | Start receiving ordered events for a session from a given sequence or from the current state. | `session_id`, `from_sequence`, `event_filter`, `projection`. | `subscription_id`, optional `session_snapshot`, `next_sequence`. |
| `session/unsubscribe` | Stop a previous session subscription. | `subscription_id`, `session_id`. | `subscription_id`, `closed`. |
| `turn/start` | Submit user input, content parts, and mentions for agent execution. If a turn is active, the client must state whether the message is normal, steer, or queue. | `session_id` or `new_session`, `submission_mode`, `active_turn_id` where applicable, `content_parts`, `mentions`, `client_message_id`, `collaboration_mode` defaulting to `build`, optional `mode_overrides`. | `session_id`, `turn_id` or `queue_item_id` or `steer_item_id`, `accepted`, `classification`, `latest_sequence`. |
| `command/exec` | Start a client-owned PTY process for Shell Mode or one-shot shell execution. | optional `session_id`, `process_id`, `program`, optional `cwd`, optional `size`. `cwd` is required when `session_id` is omitted. | `process_id`. |
| `command/exec/write` | Write base64-encoded stdin bytes to a running client-owned PTY process. | optional `session_id`, `process_id`, optional `delta_base64`, `close_stdin`. | empty success. |
| `command/exec/resize` | Resize a running PTY process. | optional `session_id`, `process_id`, `size`. | empty success. |
| `command/exec/terminate` | Terminate a running client-owned PTY process. | optional `session_id`, `process_id`. | empty success. |
| `turn/shell_command` | Removed external compatibility endpoint; the shared shell-turn execution path is internal only. | — | — |
| `message/editPrevious` | Removed external compatibility endpoint; use canonical `session/message/edit`. | — | — |
| `session/interrupt` | Request interruption of the active session turn, a Native task, or a sessionless command process. | `scope: { scope: "session", session_id }`, `scope: { scope: "task", item_id }`, or `scope: { scope: "command", process_id }`. | `interrupted`. |
| `turn/resume` | Start a continuation turn linked to an interrupted or otherwise recoverable turn. | `session_id`, `interrupted_turn_id`, `client_resume_id`, optional `resume_content_parts`, optional `resume_mentions`, optional `resume_mode`. | `session_id`, `turn_id`, `resume_of_turn_id`, `accepted`, `resume_state`, `latest_sequence`. |
| `execution/inspect` | Return active execution state so a client can show running work and let the user choose what to stop. | `session_id`, optional `include_background_processes`, optional `include_recent_output`, optional `redaction_level`. | `active_turn`, `running_tool_calls`, `pending_approvals`, `pending_questions`, `background_processes`, `latest_sequence`. |
| `backgroundProcess/stop` | Request stop for a tracked background process started by the program. | `process_id`, optional `session_id`, optional `turn_id`, `reason`, optional `stop_mode`. | `process_id`, `stop_state`, `latest_sequence`. |
| `queue/cancel` | Cancel a queued message before it starts execution. | `session_id`, `queue_item_id`, `reason`. | `queue_item_id`, `canceled`, `latest_sequence`. |
| `goal/get` | Return the current Ralph Loop goal projection for a session. | `session_id`, optional `include_history`, optional `redaction_level`. | `goal`, `goal_history` where requested, `latest_sequence`. |
| `goal/create` | Create the first goal for a session or explicitly replace a non-terminal goal. | `session_id`, `objective`, optional `token_budget`, optional `time_budget_seconds`, optional `turn_budget`, optional `replace_existing`, optional `expected_goal_id`. | `accepted`, `goal`, `replaced_goal_id` where applicable, `latest_sequence`. |
| `goal/pause` | Pause autonomous continuation without discarding the goal. | `session_id`, `goal_id`, optional `expected_goal_id`, optional `reason`. | `accepted`, `goal`, `latest_sequence`. |
| `goal/resume` | Resume a paused or blocked goal and allow continuation after eligibility checks. | `session_id`, `goal_id`, optional `expected_goal_id`, optional `resume_reason`. | `accepted`, `goal`, `continuation_eligible`, `latest_sequence`. |
| `goal/complete` | Let the user mark the goal complete directly. | `session_id`, `goal_id`, optional `expected_goal_id`, `verification_summary`. | `accepted`, `goal`, `latest_sequence`. |
| `goal/cancel` | End the goal without marking it complete. | `session_id`, `goal_id`, optional `expected_goal_id`, `reason`. | `accepted`, `goal`, `latest_sequence`. |
| `goal/clear` | Remove the current goal from normal UI projections while preserving audit records. | `session_id`, `goal_id`, optional `expected_goal_id`. | `accepted`, `cleared_goal_id`, `latest_sequence`. |
| `search/start` | Start a connection-local fuzzy-search session for one or more provider groups. | `session_id` where applicable, `client_search_id`, optional `providers`, `initial_query`, `limit`, optional `search_roots`, optional `include_indices`. | `search_id`, `providers`, `accepted`, `latest_sequence` where applicable. |
| `search/update` | Update the query for an active fuzzy-search session. | `search_id`, `query`, optional `limit`, optional `include_indices`. | `search_id`, `accepted`, `query_revision`. |
| `search/cancel` | Cancel or release an active fuzzy-search session. | `search_id`, optional `reason`. | `search_id`, `canceled`. |
| `approval/respond` | Answer a pending tool or permission approval request. | `session_id`, `turn_id`, `approval_id`, `decision`, optional `note`. | `approval_id`, `accepted`, `latest_sequence`. |
| `question/respond` | Answer a pending Plan Mode or question-tool prompt. | `session_id`, `turn_id`, `question_id`, `answers`, optional `freeform_text`. | `question_id`, `accepted`, `latest_sequence`. |
| `model/list` | Return supported and configured model projections suitable for selection UI. | `include_supported`, `configured_only`, optional `capability_filter`. | `models`, `current_model`, credential status only. |
| `model/select` | Change the session's active model binding and reasoning effort where allowed. | `session_id`, `model_binding_id`, optional `reasoning_effort`, optional `persist_default`. | `effective_model`, `metadata_update`, `latest_sequence`. |
| `config/inspect` | Return effective configuration and source information safe for client display. | `scope`, `include_sources`, optional `redaction_level`. | `effective_config_projection`, `sources`, `latest_sequence`. |
| `config/update` | Update supported configuration values where the client is authorized to do so. | `scope`, `updates`, `persistence_target`, `redaction_level`. | `accepted`, `changed_keys`, `effective_config_projection`, `latest_sequence`. |

Request methods should return explicit success or structured error results.

## Turn Input Mode Protocol Rules

The `turn/start` request carries the client-selected `collaboration_mode` for the submitted turn. Missing `collaboration_mode` must be interpreted as `build` so older clients continue to submit normal build turns.

Rules:

- `build` is the default collaboration mode and does not add mode-specific context.
- `plan` marks the submitted turn as Plan Mode. In the v1 protocol/runtime integration, the server appends hidden model context instructing the agent not to modify files or perform implementation work. This is prompt-level behavior, not a hard per-tool runtime gate.
- Queued user turns must preserve their submitted collaboration mode when they later start.
- Shell Mode commands are not encoded as `turn/start` user text.
- The TUI starts each Shell Mode submission through a one-shot `command/exec` process and receives output through `command/exec/outputDelta`.
- `command/exec` returns a start acknowledgement immediately. Process completion is reported by `command/exec/exited`.
- `command/exec` process routing is keyed by connection id, optional `session_id`, and `process_id`. Missing `session_id` is valid for startup shell commands when the client provides an explicit `cwd`.
- When `session_id` is omitted, the server must not create or activate a session. When `session_id` is present and `cwd` is omitted, the server resolves the process cwd from the session.
- `command/exec/outputDelta` and `command/exec/exited` are delivered to the owning connection for both session-bound and sessionless processes.
- The former `turn/shell_command` compatibility endpoint is removed; Shell Mode uses `command/exec` and session-owned process control uses `task/*`.

## Server Notifications

Representative server-to-client JSON-RPC notification methods:

| Method | Purpose | Payload |
|---|---|---|
| `server/status/changed` | Tell clients that server availability, lifecycle state, or capabilities changed. | `server_id`, `status`, `capabilities`, `latest_sequence`, optional `message`. |
| `session/subscription/closed` | Tell a client that a subscription ended or can no longer be continued. | `subscription_id`, `session_id`, `reason`, optional `resubscribe_hint`. |
| `approval/requested` | Ask connected clients to present an approval decision to the user. | `sequence`, `session_id`, `turn_id`, `approval_id`, `approval_kind`, `summary`, `details`, `expires_at` where applicable. |
| `question/requested` | Ask connected clients to present a question prompt to the user. | `sequence`, `session_id`, `turn_id`, `question_id`, `prompt`, `options`, `allows_freeform`, `expires_at` where applicable. |
| `config/changed` | Tell clients that effective configuration changed and dependent displays may need refresh. | `sequence`, `changed_scopes`, `changed_keys`, `source`, `safe_summary`. |
| `goal/updated` | Tell clients that the session goal was created, replaced, paused, resumed, blocked, completed, canceled, budget-limited, cleared, or updated with progress. | `sequence`, `session_id`, `goal`, `change_kind`, `source`, `source_turn_id` where applicable, `source_client_id` where applicable. |
| `goal/continuation/started` | Tell clients that an autonomous continuation turn has started for an active goal. | `sequence`, `session_id`, `goal_id`, `turn_id`, `reason`, `budget_state`. |
| `goal/budget/limited` | Tell clients that the goal stopped because a configured budget was reached. | `sequence`, `session_id`, `goal_id`, `budget_kind`, `budget_state`, `progress_summary`. |
| `search/updated` | Send a connection-local fuzzy-search result snapshot to the requesting client. | `search_id`, `query`, `query_revision`, `provider_groups`, `is_complete`, optional `degraded_state`. |
| `search/completed` | Tell the requesting client all active providers completed for the current query. | `search_id`, `query`, `query_revision`, `provider_statuses`. |
| `search/failed` | Tell the requesting client a search session or provider failed. | `search_id`, `query`, `provider`, `error`, `recoverable`. |
| `session/started`, `session/title/updated`, `session/status/changed`, `session/archived`, `session/unarchived`, `session/closed` | Reuse existing concrete session lifecycle notification methods for subscribed clients. | Existing session payload plus sequence/session identity fields. |
| `session/compaction/started`, `session/compaction/completed`, `session/compaction/failed` | Reuse existing concrete session compaction notification methods. | Existing compaction payload plus sequence/session identity fields. |
| `turn/started`, `turn/completed`, `turn/interrupted`, `turn/failed` | Reuse existing concrete turn lifecycle notification methods. | Existing turn payload plus sequence/session/turn identity fields. |
| `turn/plan/updated`, `turn/diff/updated`, `turn/usage/updated` | Reuse existing concrete turn projection notification methods. | Existing plan, diff, or usage payload. |
| `inputQueue/updated`, `steer/accepted` | Reuse existing concrete active-turn input notification methods. | Existing queue or steer payload. |
| `item/started`, `item/completed` | Reuse existing concrete item lifecycle notification methods. | Existing item payload plus item context. |
| `item/agentMessage/delta`, `item/reasoning/summaryTextDelta`, `item/reasoning/textDelta`, `item/commandExecution/outputDelta`, `item/fileChange/outputDelta`, `item/plan/delta` | Reuse existing concrete streaming delta notification methods. | Existing item delta payload plus item context. |
| `command/exec/outputDelta` | Stream base64 PTY bytes for a client-owned Shell Mode process. | optional `session_id`, `process_id`, `stream`, `delta_base64`. |
| `command/exec/exited` | Report completion of a client-owned Shell Mode process. | optional `session_id`, `process_id`, optional `exit_code`. |
| `serverRequest/resolved` | Reuse existing concrete pending server-request resolution notification method. | Existing request resolution payload. |

Notifications should include sequence numbers sufficient for clients to order events and request catch-up after reconnect.

## Sequencing And Catch-Up

The server should assign a monotonic `session_sequence` for each session. Every session-scoped or turn-scoped notification for that session should carry this sequence, including concrete methods such as `session/started`, `turn/started`, `item/started`, and `item/agentMessage/delta`.

Rules:

- Clients should pass `from_sequence` when subscribing or reconnecting.
- The server should deliver missed events after `from_sequence` when those events are still available.
- If the requested sequence is too old, unknown, or compacted away, the server should send a fresh `session_loaded` snapshot and continue from the snapshot's `latest_sequence`.
- Clients should treat events as idempotent by sequence and event identity.
- Client-generated request ids such as `client_message_id` should make retries safe after transient disconnects.
- Ordering is authoritative within a session. Cross-session global ordering is not required for normal client rendering.

## Server-Client Event Payloads

Server-client event payloads should be shaped for UI responsiveness and recovery, not for durable storage.

Representative server-client event kinds:

| Event Kind | Purpose | Payload Content |
|---|---|---|
| `session_loaded` | Provide a session projection after open, subscribe, or reconnect. | `session_id`, `metadata`, `visible_turns`, `pending_items`, `active_plan`, `active_goal`, `latest_sequence`. |
| `metadata_updated` | Report a change to session metadata such as model, reasoning, mode, persona, permission profile, workspace, or usage totals. | `session_id`, `metadata_patch`, `effective_metadata`, `source_client_id`, `sequence`. |
| `plan_updated` | Report creation or update of visible plan/to-do state from the plan tool. | `session_id`, `plan_id`, `operation`, `plan_status`, `items`, `changed_item_ids`, `source_turn_id`, `timestamp`. |
| `goal_updated` | Report creation, replacement, status transition, progress, blocker, verification, budget, or clear state for the current Ralph Loop goal. | `session_id`, `goal_id`, `operation`, `status`, `objective_preview`, `progress_summary`, `blocker_summary`, `verification_summary`, `budget_state`, `source`, `source_turn_id`, `timestamp`. |
| `goal_continuation_started` | Report that the server started an autonomous continuation turn for an active goal. | `session_id`, `goal_id`, `turn_id`, `reason`, `budget_state`, `timestamp`. |
| `goal_budget_limited` | Report that goal continuation stopped because a configured budget was reached. | `session_id`, `goal_id`, `budget_kind`, `budget_state`, `progress_summary`, `timestamp`. |
| `search_updated` | Report a live fuzzy-search snapshot for a connection-local search session. | `search_id`, `query`, `query_revision`, `groups`, `matches`, `is_complete`, `timestamp`. |
| `search_completed` | Report that all search providers completed for the active query. | `search_id`, `query`, `query_revision`, `provider_statuses`, `timestamp`. |
| `search_failed` | Report that a fuzzy-search provider or session failed or degraded. | `search_id`, `query`, `provider`, `error`, `recoverable`, `timestamp`. |
| `turn_started` | Tell all subscribed clients that a new turn has begun. | `session_id`, `turn_id`, `status`, `submitted_by_client_id`, `user_item_id`, `started_at`. |
| `turn_resumed` | Tell subscribed clients that an interrupted or recoverable turn has been resumed by a linked continuation turn. | `session_id`, `interrupted_turn_id`, `resume_turn_id`, `resume_mode`, `submitted_by_client_id`, `timestamp`. |
| `turn_status_changed` | Report a turn moving between running, waiting, completed, failed, or interrupted states. | `session_id`, `turn_id`, `previous_status`, `status`, `reason`, `timestamp`. |
| `turn_diff_updated` | Report the current display diff for files changed by the turn. This is a client-display projection, not the authoritative restore record. | `session_id`, `turn_id`, `change_set_id`, `diff_format`, `diff_ref` or `inline_diff`, `changed_files`, `is_complete`, `timestamp`. |
| `item_started` | Create or display a logical transcript item. | `session_id`, `turn_id`, `item_id`, `kind`, `role`, `visibility`, `initial_content`, `mentions`, `created_at`. |
| `item_content_update` | Apply a live content update to an existing item. | `session_id`, `turn_id`, `item_id`, `content_part_index`, `operation`, `text` or `content_ref`, `is_coalesced`, `timestamp`. |
| `item_completed` | Mark an item complete and provide final display metadata. | `session_id`, `turn_id`, `item_id`, `final_status`, `content_hash`, `completed_at`. |
| `item_failed` | Mark an item failed while preserving any partial content already sent. | `session_id`, `turn_id`, `item_id`, `error`, `recoverable`, `timestamp`. |
| `message_edit_recorded` | Show that an immediately previous message edit was accepted. | `session_id`, `edit_id`, `target_message_id`, `replacement_message_id`, `edit_state`, `content_preview`, `mentions`, `timestamp`. |
| `turn_superseded` | Mark a previous turn as superseded by an edited message continuation while keeping it auditable. | `session_id`, `superseded_turn_id`, `replacement_turn_id`, `edit_id`, `reason`, `timestamp`. |
| `workspace_restore_started` | Show that the server is attempting to restore files changed by a superseded turn. | `session_id`, `edit_id`, `superseded_turn_id`, `checkpoint_id`, `candidate_files`, `restore_policy`, `timestamp`. |
| `workspace_restore_completed` | Report the outcome of restoring files changed by a superseded turn. | `session_id`, `edit_id`, `superseded_turn_id`, `restored_files`, `skipped_files`, `unsupported_files`, `failed_files`, `current_state_kept`, `timestamp`. |
| `steer_added` | Show that a steer message was accepted for an active turn. | `session_id`, `turn_id`, `steer_item_id`, `content_preview`, `application_state`, `timestamp`. |
| `steer_reclassified` | Report that a requested steer could not affect the active turn and was queued, rejected, or otherwise resolved. | `session_id`, `turn_id`, `steer_item_id`, `new_classification`, `reason`, `queue_item_id` where applicable. |
| `queue_item_added` | Show that a queued message was accepted. | `session_id`, `queue_item_id`, `position`, `content_preview`, `created_at`. |
| `queue_item_started` | Show that a queued message has become the next executing turn. | `session_id`, `queue_item_id`, `turn_id`, `started_at`. |
| `queue_item_canceled` | Show that a queued message was canceled before execution. | `session_id`, `queue_item_id`, `reason`, `timestamp`. |
| `tool_call_started` | Show that a tool call has begun or is awaiting approval. | `session_id`, `turn_id`, `item_id`, `tool_call_id`, `tool_name`, `command_description` for command tools, `arguments_preview`, `approval_state`, `safety_state`, optional `parent_tool_call_id`, optional `parallel_group_id`, optional `child_index`, optional `group_child_count`. |
| `tool_call_updated` | Update tool call progress, streaming output preview, or status. | `session_id`, `turn_id`, `tool_call_id`, `status`, `progress`, `output_preview`, `redaction_state`, `safety_notice`, `timestamp`. |
| `tool_call_completed` | Show final tool result state. | `session_id`, `turn_id`, `tool_call_id`, `status`, `result_summary`, `structured_status`, `output_ref`, `redaction_state`, `safety_notice`, `completed_at`. |
| `background_process_updated` | Show state for a tracked background process started by the program. | `process_id`, `session_id`, `turn_id`, `command_label`, `status`, `runtime`, `recent_output_ref`, `stop_state`, `timestamp`. |
| `approval_resolved` | Report the final state of an approval request to all subscribed clients. | `session_id`, `turn_id`, `approval_id`, `decision`, `resolved_by_client_id`, `resolved_at`. |
| `question_resolved` | Report the final state of a question request to all subscribed clients. | `session_id`, `turn_id`, `question_id`, `answer_summary`, `resolved_by_client_id`, `resolved_at`. |
| `usage_updated` | Update token and cost-related display information. | `session_id`, `turn_id`, `invocation_id`, `usage_delta`, `usage_totals`. |
| `context_updated` | Report active context changes, compaction, or token pressure. | `session_id`, `context_id`, `token_estimate`, `effective_context_limit`, `compaction_status`, `compaction_trigger_source` where applicable. |
| `session_export_ready` | Report that an export request completed or failed. | `export_id`, `status`, `download_ref`, `error` where applicable. |
| `error_reported` | Report recoverable or terminal errors tied to a session, turn, item, or server operation. | `scope`, `phase`, `session_id`, `turn_id`, `item_id`, `code`, `message`, `recoverable`, `retry_state`, `retry_after`, `provider_error_ref`, `partial_state`, `recovery_actions`, `details_ref`. |

`item_content_update` is a live client event and may be coalesced or throttled. It is not the same as a durable JSONL storage event.

## Cross-Client Broadcast Behavior

When one client submits a user message, the server must persist the accepted user input and broadcast the resulting session and turn updates to every client subscribed to that session.

All clients, including the client that initiated the request, should receive the canonical server events. Clients may optimistically render local input, but they must reconcile that display against the server-confirmed `turn_started`, `item_started`, and later item events.

Examples:

- If the TUI submits `turn/start`, desktop and IDE clients subscribed to the same session receive the new user item and turn state.
- If the desktop client answers an approval request, the TUI and IDE clients receive the approval state update and any resumed turn events.
- If the server streams assistant output, every subscribed client receives ordered `item_content_update` events for the same item.
- If the agent updates the plan tool, every subscribed client receives `plan_updated` for the same active plan state.
- If one client creates, pauses, resumes, completes, cancels, or clears a goal, every subscribed client receives the canonical `goal_updated` event.
- If the server starts an autonomous goal continuation, every subscribed client receives `goal_continuation_started` and the normal turn lifecycle events for that continuation turn.
- If a turn changes files through structured mutating tools, subscribed clients may receive `turn_diff_updated` events for review display.
- If one client interrupts or resumes a turn, every subscribed client receives the canonical turn status and resume events.
- If one client edits the immediately previous message, every subscribed client receives the edit event and any superseded or replacement turn events.

## Immediate Previous Message Editing

The protocol supports editing only the immediately preceding eligible user-authored message in the current session branch.

Eligibility rules:

- The server is authoritative for identifying the current branch's immediately previous eligible message.
- `message/editPrevious` must reject stale requests when `expected_target_message_id` is not the current eligible message.
- Direct editing of older historical messages must be rejected with a structured error that points the client toward session forking.
- Accepted edits must be append-only from the protocol perspective: the server records an edit and broadcasts canonical replacement state instead of mutating earlier events in place.
- The server/core is authoritative for workspace restoration. Clients may choose an allowed `workspace_restore_policy`, but clients must not be required to apply inverse patches or mutate the workspace to make message editing correct.

Execution rules:

- If the target message belongs to a completed, failed, or interrupted latest turn, the server should attempt workspace restoration for files changed by that turn, create a replacement user item and a replacement continuation turn, then mark the original turn as superseded.
- Workspace restoration should run before the replacement turn begins unless the edit is staged for later execution.
- `workspace_restore_policy` should allow the client to request the default safe restore behavior, skip restoration, or use another explicitly supported policy. The default safe behavior preserves current file contents when divergence is detected.
- Workspace restoration should use core-owned per-turn change sets, inverse records, content snapshots, or internal checkpoints. Client-visible unified diffs may help users review changes, but they are not the authoritative restore state.
- For each file changed by the superseded turn, the server should restore the pre-turn state when the current file state still matches the expected post-turn state or another safe restore predicate.
- If a changed file has diverged after the superseded turn, the server must skip restoration for that file, preserve the current file state, and report the skip in `workspace_restore_completed`.
- File changes from structured tools such as `write` and `apply_patch` should use captured before/after state or inverse operations.
- File changes from shell commands should be restored only when a reliable turn-level checkpoint or attribution record exists.
- A git-based hidden checkpoint or ghost commit may be used internally, but the protocol must expose restoration outcome rather than git implementation details. It must not publish, stage, or rewrite user-visible git history unless the user explicitly requests that.
- If the target message is a queued message that has not started, the server may update the queue item's effective content through an edit record and preserve the original revision for audit.
- If the target message belongs to an active running turn, the server must not mutate the already-started model or tool execution. It must reject the edit or require an interruption-oriented `edit_mode`; clients may offer `steer` as the lower-friction alternative.
- If a superseded turn produced non-file tool side effects, those side effects remain visible in the superseded turn. Message editing does not imply rollback for external APIs, processes, network actions, published git operations, or other non-file effects.

Broadcast rules:

- Every accepted edit must emit `message_edit_recorded` to subscribed clients.
- If workspace restoration is attempted, subscribed clients must receive `workspace_restore_started` and `workspace_restore_completed`.
- If a completed latest turn is replaced, subscribed clients must also receive `turn_superseded` and the normal events for the replacement turn.
- Clients may optimistically show an edit draft, but they must reconcile it against the server-confirmed `message_edit_recorded` event.
- `turn_diff_updated` events may be coalesced or replaced by later diff updates. Clients should treat them as display state and should not infer restore completion from them.

## Interrupt And Resume Protocol Rules

The server is authoritative for interrupt and resume state. Clients request control actions, but they must reconcile local UI against server-confirmed `turn_status_changed`, `tool_call_updated`, `background_process_updated`, and `turn_resumed` events.

Rules:

- `session/interrupt` must return after the server accepts or rejects the request. Turn cancellation continues through the existing runtime cancellation path; command termination remains scoped to the requesting connection and session.
- Accepted interruption should move the target into stopping, interrupted, completed-before-interrupt, failed, or cleanup-pending state.
- If the target turn is already terminal, the server should return an idempotent terminal result or a structured stale-state error.
- `execution/inspect` should return enough active work state for clients to show running model invocation, running tools, pending prompts, and tracked background processes without exposing secrets.
- `backgroundProcess/stop` should only target processes started and tracked by the program.
- `turn/resume` should create a linked continuation turn rather than mutating the interrupted turn in place.
- Resume requests should be rejected or degraded with a warning when required context, workspace state, or permission state is unavailable.
- Resumed turns must use the normal execution engine and normal safety policy.

## Goal Protocol Rules

The server is authoritative for Ralph Loop goal state. Clients request user-owned goal mutations, and clients render canonical `goal_updated` events.

Rules:

- Goal requests must return promptly after acceptance or rejection. They must not wait for an autonomous continuation turn to complete.
- `goal/create` should reject accidental replacement of an existing non-terminal goal unless `replace_existing` or an equivalent confirmation is explicit.
- Goal mutations should include `expected_goal_id` where the client has one. If the goal was replaced or reached a terminal state, stale mutations should fail safely or become no-ops with a structured stale-state result.
- Model-originated goal updates must not use client-owned mutation methods. They should flow through the narrow model-facing goal tool defined by `L2-DES-GOAL-001` and then broadcast as `goal_updated`.
- If a user pauses, cancels, clears, or replaces a goal while a turn is active, that mutation affects future continuation and future context assembly. It must not rewrite a model request that has already started.
- If `goal/resume` makes a goal active, the server should report whether continuation is currently eligible. If it is not eligible, the reason should be visible in the response or later events.
- If the server starts a continuation, it should emit `goal_continuation_started` before or alongside the normal `turn_started` event.
- If budget is reached, the server should emit `goal_budget_limited` and `goal_updated` so clients can explain that the goal stopped without implying verified completion.
- Plan Mode suppresses autonomous goal continuation. Goal state remains viewable and user-controllable while Plan Mode is active.

## Search Protocol Rules

Fuzzy-search sessions are live client interaction state. They are not transcript turns, durable session events, or cross-client broadcasts by default.

Rules:

- `search/start` must return promptly with a server-owned `search_id` or a structured rejection.
- `search/updated`, `search/completed`, and `search/failed` are sent only to the requesting client connection unless a later requirement defines shared picker state.
- Connection-local search notifications must be delivered to the requesting connection even when the only active notification subscriptions are session-scoped (`session_id=Some(...)`). Search snapshots carry no `session_id` and must not depend on `events/subscribe` session matching.
- `search/update` should supersede prior query work for the same `search_id`.
- Search query text is the literal keyword after the client prefix. It must not require or encode provider selectors such as file, skill, or MCP.
- If `providers` is omitted, the server should use the default provider set for prefixed input.
- Search notifications must include enough query identity, such as `query_revision`, for clients to ignore stale snapshots.
- `search/cancel` should release worker state and stop provider work where possible.
- Provider groups must preserve result type so clients can render skills, MCP entries, files, sessions, commands, or future providers distinctly.
- File search results should use workspace-relative paths where possible and include explicit file or directory type.
- Search results must respect workspace, permission, privacy, ignore, and exclude policy before being sent to clients.
- Search events are live UI projections. They must not be written as session transcript records unless a user later submits a message containing a selected mention.

## Tool And Plan Protocol Rules

Tool calls are requested by the model and executed by the server-owned tool supervisor. Clients observe canonical tool and plan events; they do not decide whether a model-requested tool call is valid except when an approval or question response is explicitly requested.

Rules:

- Tool state should be reported through `tool_call_started`, `tool_call_updated`, and `tool_call_completed`.
- `tool_call_started` should include `command_description` for shell or command execution tools.
- A tool unavailable due to mode, permission, or missing configuration should complete with a structured blocked or unavailable result rather than disappearing.
- Tool completion should include a factual natural-language `result_summary` alongside structured status fields such as exit code, HTTP status, process id, or changed-file counts.
- Plan tool updates should be reported through `plan_updated`, not only as assistant text.
- `plan_updated` should carry the complete current plan projection or enough patch data for clients to reconstruct it from prior canonical plan state.
- The plan tool must not expose private model reasoning. Plan item text should be concise, user-visible task state.
- In Normal Mode, question-tool attempts must be rejected before `question/requested` is emitted.
- In Plan Mode, mutating tools must be blocked before execution and should produce a structured blocked result if requested.
- `multi_tool_use` child calls must still produce per-tool events and must not bypass validation, approval, or mode gates.
- `multi_tool_use` parent and child events should include `parallel_group_id`; child events should also include `parent_tool_call_id` and `child_index` so clients can render group-level and child-level activity independently.

## Approval And Question Resolution

Approval and question requests are single-resolution prompts. Multiple clients may display the same prompt, but only the first accepted response should resolve it.

Rules:

- The server owns approval and question state.
- Approval requests are the protocol surface for `L1-REQ-APP-003` safety escalation. A client may present and answer a prompt, but the server decides whether the answer satisfies the current permission, sandbox, mode, and safety policy.
- A successful `approval/respond` or `question/respond` resolves the prompt and broadcasts `approval_resolved` or `question_resolved`.
- If another client answers after the prompt has been resolved, the server must reject the request with a structured stale-state error such as `already_resolved`.
- Approval responses must be scoped to the identified `approval_id` and must not be treated as an unlimited permission grant for later unrelated work.
- `question/requested` must only be emitted when Plan Mode or another explicit requirement allows the question tool.
- In Normal Mode, the server must reject question-tool attempts before emitting `question/requested`.

## Future Destructive Deletion And Forks

The Devo-native client-server protocol does not currently define a destructive session deletion request. ACP `session/delete` is the supported session-history removal method and only specifies user-facing removal from `session/list` results.

If a future Devo-native destructive deletion endpoint is added, it must preserve user-visible consistency for forks.

Rules:

- The deletion endpoint must report fork descendants before destructive deletion when descendants exist.
- Deleting a parent session must not make surviving forked sessions unusable.
- If a fork survives parent deletion, inherited history required by that fork must remain available through a replayable inherited-history segment. That segment may be backed by protected shared records, materialized fork history, or another explicit retention mechanism.
- The parent session link in a fork is provenance and navigation metadata. It must not be the sole content pointer required to replay inherited history.
- After deletion, the parent session link may be non-dereferenceable. Clients must treat parent navigation failure as distinct from inherited-history loss.
- Before a parent session is made inaccessible, the deletion endpoint must either preserve the inherited segment for each surviving fork, materialize the inherited segment into the fork, or reject deletion until the user chooses another policy.
- Fork indicators should show parent deleted or unavailable when navigation to the parent can no longer work, while keeping origin metadata visible.
- Hard deletion of records still referenced by surviving forks must be blocked unless those forks first receive replayable inherited-history segments or the user explicitly requests cascade deletion of the dependent forks where supported.

Persistent memory is core-maintained internal state. Session deletion may cause core memory maintenance internally, but the client-server protocol must not expose per-memory deletion decisions, linked memory lists, or memory-management prompts.

## Provider Event Boundary

Provider/core events are internal to agent execution. They may be more granular than both server-client events and durable JSONL events.

Representative provider/core events:

- `llm_request_started`
- `reasoning_started`
- `reasoning_delta`
- `reasoning_completed`
- `assistant_response_started`
- `assistant_response_delta`
- `assistant_response_completed`
- `tool_call_started`
- `tool_call_arguments_delta`
- `tool_call_completed`
- `usage_received`
- `llm_request_completed`
- `llm_request_failed`

The server should normalize provider/core events into:

- Durable JSONL events for persistence.
- Server-client events for live display.
- Runtime control events for orchestration.

## Multi-Client State

The server owns sessions, turns, approvals, model invocation, tool execution, context assembly, and persistence.

Clients should not create independent server state when an existing local server is available. Multiple connected clients should subscribe to the same session and receive ordered event streams for the same underlying turns and items.

If a client disconnects, the server continues owning active work subject to user-configured lifecycle policy. A reconnecting client should resubscribe and receive either missed server-client events or a fresh projection of the current durable session state.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-001 | 1 | specs/L1/L1-REQ-APP-001-client-server-arch.md | Defines protocol, transport, and process ownership for shared clients. |
| related-to | L1-REQ-APP-002 | 1 | specs/L1/L1-REQ-APP-002-persistence.md | Reconnect and catch-up behavior depends on durable session state. |
| related-to | L1-REQ-APP-003 | 1 | specs/L1/L1-REQ-APP-003-safety.md | Defines approval request and response protocol behavior for actions outside current permission boundaries. |
| related-to | L1-REQ-APP-010 | 1 | specs/L1/L1-REQ-APP-010-configuration.md | Defines configuration inspection and update protocol behavior. |
| related-to | L1-REQ-APP-011 | 1 | specs/L1/L1-REQ-APP-011-error-recovery.md | Defines error and retry event payload requirements. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | Defines export, deletion, and credential-safe projection behavior. |
| related-to | L1-REQ-TUI-008 | 1 | specs/L1/L1-REQ-TUI-008-terminal-command-prefix.md | Defines the protocol surface for user-submitted Shell Mode commands. |
| related-to | L1-REQ-TUI-009 | 1 | specs/L1/L1-REQ-TUI-009-session-input-modes.md | Defines per-turn input-mode metadata and Shell command submission. |
| related-to | L1-REQ-APP-006 | 1 | specs/L1/L1-REQ-APP-006-fuzzysearch.md | Defines fuzzy-search request and live result event behavior. |
| related-to | L1-REQ-CLIENT-004 | 1 | specs/L1/L1-REQ-CLIENT-004-prefixed-input-actions.md | Defines protocol support for `@` prefix fuzzy-search sessions. |
| related-to | L1-REQ-AGENT-001 | 1 | specs/L1/L1-REQ-AGENT-001-execution-workflow.md | Exposes execution lifecycle requests and events to clients. |
| related-to | L1-REQ-AGENT-002 | 1 | specs/L1/L1-REQ-AGENT-002-interrupt-resume.md | Defines interrupt, resume, active-work inspection, and background stop protocol surfaces. |
| related-to | L1-REQ-AGENT-003 | 1 | specs/L1/L1-REQ-AGENT-003-task-planning.md | Exposes plan tool updates as client-visible plan state. |
| related-to | L1-REQ-AGENT-005 | 1 | specs/L1/L1-REQ-AGENT-005-plan-mode.md | Enforces question notification constraints for Plan Mode. |
| related-to | L1-REQ-CHANGE-001 | 1 | specs/L1/L1-REQ-CHANGE-001-rollback-and-recovery.md | Defines display diff events plus server-owned restoration events and outcomes for superseded-turn file rollback. |
| related-to | L1-REQ-CONV-001 | 1 | specs/L1/L1-REQ-CONV-001-session-lifecycle.md | Clients open, subscribe to, and resume sessions through the protocol. |
| related-to | L1-REQ-CONV-002 | 1 | specs/L1/L1-REQ-CONV-002-turn-lifecycle.md | Clients observe turn lifecycle events through the protocol. |
| related-to | L1-REQ-CONV-003 | 1 | specs/L1/L1-REQ-CONV-003-active-turn-message-handling.md | Defines steer and queue request and event behavior. |
| related-to | L1-REQ-CONV-004 | 1 | specs/L1/L1-REQ-CONV-004-session-forking.md | Defines fork, delete, and parent-unavailable behavior. |
| related-to | L1-REQ-CONV-005 | 1 | specs/L1/L1-REQ-CONV-005-immediate-message-editing.md | Defines immediate previous message edit protocol behavior. |
| related-to | L1-REQ-EDIT-001 | 1 | specs/L1/L1-REQ-EDIT-001-file-editing-workflow.md | Structured file-editing tools provide restoration data for superseded turns. |
| related-to | L1-REQ-GOAL-001 | 1 | specs/L1/L1-REQ-GOAL-001-ralph-loop.md | Defines goal control requests, goal events, and autonomous continuation visibility. |
| related-to | L1-REQ-GIT-001 | 1 | specs/L1/L1-REQ-GIT-001-change-management.md | Git checkpoints may support restoration without user-visible git history changes. |
| related-to | L1-REQ-TOOL-001 | 1 | specs/L1/L1-REQ-TOOL-001-safety.md | Defines redaction and safety fields in tool events. |
| related-to | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | Exposes built-in tool lifecycle and plan tool updates to clients. |
| related-to | L1-REQ-TOOL-005 | 1 | specs/L1/L1-REQ-TOOL-005-background-process-management.md | Exposes tracked background process state and stop requests. |
| related-to | L1-REQ-MEM-001 | 1 | specs/L1/L1-REQ-MEM-001-persistent-memory.md | Excludes persistent memory from routine client-server protocol methods and notifications. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | The protocol exposes execution engine state to clients. |
| related-to | L2-DES-AGENT-002 | 1 | specs/L2/agent/L2-DES-AGENT-002-interrupt-resume-control.md | The protocol exposes interrupt and resume control actions. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | The protocol exposes tool and plan state from the built-in tool system. |
| related-to | L2-DES-TOOL-002 | 1 | specs/L2/tool/L2-DES-TOOL-002-parallel-tool-orchestration.md | Parallel tool groups require parent and child event identifiers for client rendering and replay. |
| related-to | L2-DES-GOAL-001 | 1 | specs/L2/goal/L2-DES-GOAL-001-ralph-loop-goals.md | The protocol exposes user-owned goal controls and canonical goal notifications. |
| related-to | L2-DES-APP-006 | 1 | specs/L2/app/L2-DES-APP-006-fuzzy-search-architecture.md | The protocol exposes fuzzy-search sessions and live result snapshots. |
| specified-by | L3-BEH-CORE-011 | 1 | specs/L3/core/L3-BEH-CORE-011-session-forking-retention.md | L3 defines concrete behavior behind `session/fork`, inherited segment actions, future destructive deletion safeguards, and parent-unavailable fork projection. |
| specified-by | L3-BEH-CORE-012 | 1 | specs/L3/core/L3-BEH-CORE-012-message-edit-workspace-restore.md | L3 defines concrete behavior behind `message/editPrevious`, restore policy, workspace restore events, superseded turns, and replacement turn sequencing. |
| related-to | L2-DES-CLIENT-002 | 1 | specs/L2/client/L2-DES-CLIENT-002-prefixed-input-actions.md | The protocol supports server-owned search providers used by prefixed input. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable session events are distinct from live server-client protocol events. |
| related-to | L2-DES-APP-005 | 2 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | `config/inspect` and `config/update` operate on safe projections and updates derived from the `config.toml` and `auth.json` schemas. |
| specified-by | L3-BEH-PROTOCOL-001 | 3 | specs/L3/protocol/L3-BEH-PROTOCOL-001-jsonrpc-protocol.md | L3 defines JSON-RPC envelope validation, session subscription, sequencing, broadcasting, turn submission, idempotency, approval races, fork/delete, and message edit requests. |
| specified-by | L3-BEH-SERVER-001 | 4 | specs/L3/server/L3-BEH-SERVER-001-server-runtime-transport.md | L3 defines server runtime transport ownership and event broadcast behavior. |
| specified-by | L3-BEH-CLIENT-001 | 2 | specs/L3/client/L3-BEH-CLIENT-001-connection-subscription.md | L3 defines client connection, subscription, reconnection, and catch-up behavior. |

## References

- JSON-RPC 2.0 specification.
- Visual Studio Code Web Extensions documentation.
- Visual Studio Code Extension Host documentation.
- Visual Studio Code Language Server Extension Guide.
- `microsoft/vscode-languageserver-node` transport documentation.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial client/server protocol, transport, provider-event boundary, and multi-client server ownership design. |
| 1 | 2026-05-25 | Human | Refinement | Linked configuration protocol methods to the concrete `config.toml` and `auth.json` schemas. |
| 1 | 2026-05-22 | Human | Refinement | Made WebSocket the concrete transport, removed the per-workspace caveat, expanded request and notification descriptions, and added cross-client broadcast behavior. |
| 1 | 2026-05-22 | Human | Refinement | Added steer and queue protocol behavior, deletion/export, fork deletion policy, sequencing, approval races, Plan Mode guards, error recovery fields, and tool safety fields. |
| 1 | 2026-05-22 | Human | Refinement | Added immediate previous message editing request, events, and branch-safe protocol rules. |
| 1 | 2026-05-22 | Human | Refinement | Clarified that surviving forks use replayable inherited-history segments rather than relying on parent-session links after deletion. |
| 1 | 2026-05-22 | Human | Refinement | Added deletion response visibility for inherited segment preservation actions and non-dereferenceable parent links. |
| 1 | 2026-05-22 | Human | Refinement | Added workspace restoration request fields and events for immediate message editing. |
| 1 | 2026-05-22 | Human | Refinement | Removed persistent-memory management methods and notifications from the client-server protocol. |
| 1 | 2026-05-22 | Human | Refinement | Clarified that turn diffs are client-display projections and workspace restoration remains server/core-owned. |
| 1 | 2026-05-22 | Human | Refinement | Added execution inspection, interrupt, resume, and background-process stop protocol surfaces. |
| 1 | 2026-05-22 | Human | Refinement | Added plan update events and tool protocol rules for the built-in tool system. |
| 1 | 2026-05-22 | Human | Refinement | Added command descriptions and natural-language result summaries to tool protocol projections. |
| 1 | 2026-05-23 | Human | Refinement | Added Ralph Loop goal requests, notifications, server-client events, and protocol rules. |
| 1 | 2026-05-25 | Human | Refinement | Added connection-local fuzzy-search requests, notifications, event payloads, and protocol rules. |
| 1 | 2026-05-25 | Human | Refinement | Clarified that search query text is independent from provider selection for prefixed input. |
| 1 | 2026-05-25 | Human | Refinement | Added compaction trigger source to context update events for transcript-area compaction notices. |
| 1 | 2026-05-25 | Assistant | Refinement | Linked approval protocol behavior to `L1-REQ-APP-003` application safety. |
| 1 | 2026-05-26 | Assistant | Refinement | Added parallel tool parent/child event identity fields for `multi_tool_use`. |
| 2 | 2026-05-27 | Human | Refinement | Changed JSON-RPC method names from dot separators to slash separators and aligned server notification method names with existing protocol event names where available. |
| 2 | 2026-06-08 | Assistant | Refinement | Added `collaboration_mode` to `turn/start`, defined `command/exec` dynamic PTY process control for Shell Mode, allowed sessionless startup shell commands with explicit `cwd`, and retained `turn/shell_command` as a legacy compatibility path. |
