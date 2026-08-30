---
artifact_id: L2-DES-CONV-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-06-23
---

# L2-DES-CONV-001 — Session JSONL Data Model

## Purpose

Refine session, transcript, turn, item, and active-context requirements into a durable append-only data model suitable for server processing, client rendering, and crash recovery.

## Background / Context

The program needs session data in both server logic and client interfaces such as the TUI. Session data must also be persisted. JSONL is the preferred durable storage shape because each line can represent an append-only event or snapshot, which supports crash recovery and preserves stable context prefixes for token efficiency.

A session has two major conceptual regions:

- Metadata: durable session state, configuration references, workspace identity, active mode, persona, token usage, and active-context state.
- Transcript: user-visible and auditable conversational history made from turns and items.

The full transcript can grow without bound from the user's perspective. The active model context is finite and should reference transcript records rather than duplicating them.

## Source Requirements

- `L1-REQ-CONV-001` requires durable session lifecycle behavior.
- `L1-REQ-CONV-002` requires auditable turn lifecycle behavior.
- `L1-REQ-CONV-003` requires durable and visible `steer` and `queue` handling during active turns.
- `L1-REQ-CONV-004` requires session forking from a specific turn and fork traceability.
- `L1-REQ-CONV-005` requires append-only editing of the immediately preceding eligible user-authored message.
- `L1-REQ-AGENT-001` requires durable enough execution history for review and recovery.
- `L1-REQ-AGENT-002` requires completed steps, outputs, and file-change state to be preserved after interruption and resume.
- `L1-REQ-AGENT-003` requires visible task planning with status updates.
- `L1-REQ-CHANGE-001` requires rollback and recovery behavior for file changes.
- `L1-REQ-EDIT-001` requires file edits to be reviewable and recoverable.
- `L1-REQ-GOAL-001` requires durable Ralph Loop goal state across turns and recoverable session resumes.
- `L1-REQ-GIT-001` constrains git-oriented change management.
- `L1-REQ-APP-012` requires data ownership controls for user-visible stored data and model-visible context.
- `L1-REQ-MEM-001` defines persistent memory as core-maintained internal state.
- `L1-REQ-CONTEXT-001` requires useful model context management across long-running sessions.
- `L1-REQ-CONTEXT-003` requires context compression when context approaches model limits.
- `L1-REQ-INPUT-001` requires attachments and multimodal input as first-class task context.
- `L1-REQ-TUI-003` requires transcript review and audit behavior.
- `L1-REQ-LLM-001` requires token-efficient stable context construction.
- `L1-REQ-MODEL-001` and `L2-DES-MODEL-001` define model and model-binding references used by session metadata.
- `L1-REQ-TOOL-002` and `L2-DES-TOOL-001` define built-in tool and plan-tool records.

## Design Requirement

The durable session file should be an append-only JSONL log. The log should contain versioned records that can rebuild:

- Session metadata.
- Transcript turns and items.
- Coalesced streaming content segments.
- Usage deltas and totals.
- Active context snapshots.
- Context compression outputs.

The server may materialize indexed or cached runtime state from the JSONL log, but the JSONL log remains the durable source of truth.

## Event Planes

The program should distinguish three event planes:

1. Provider/core events: high-frequency runtime events emitted while an LLM provider streams a response.
2. Server-client events: live protocol events sent to connected clients for rendering and interaction.
3. Durable JSONL events: compact replay records persisted to session storage.

These planes may carry related information, but they should not share the same event granularity.

Provider/core events may include fine-grained deltas such as reasoning text deltas, assistant text deltas, and partial tool-call argument deltas. These events are useful for runtime orchestration and live rendering, but they are too frequent to persist directly as one JSONL record per provider delta.

Server-client events may include live content updates for responsiveness. The server may coalesce or throttle these updates before sending them to clients.

Durable JSONL events should use replay-friendly records that preserve session state without storing every provider streaming delta.

## Durable Record Categories

Conceptual JSONL record categories:

- `session_created`
- `session_forked`
- `session_metadata_updated`
- `session_deleted`
- `plan_created`
- `plan_updated`
- `goal_created`
- `goal_replaced`
- `goal_status_changed`
- `goal_budget_accounted`
- `goal_progress_recorded`
- `goal_context_snapshot_recorded`
- `goal_cleared`
- `turn_started`
- `turn_interrupt_requested`
- `turn_completed`
- `turn_failed`
- `turn_interrupted`
- `turn_resume_started`
- `turn_superseded`
- `item_started`
- `item_content_appended`
- `item_completed`
- `item_failed`
- `message_edit_recorded`
- `turn_workspace_checkpoint_recorded`
- `turn_workspace_change_recorded`
- `turn_workspace_diff_recorded`
- `turn_workspace_restore_started`
- `turn_workspace_restore_completed`
- `steer_recorded`
- `queue_item_recorded`
- `queue_item_resolved`
- `usage_recorded`
- `memory_link_recorded`
- `context_snapshot_recorded`
- `context_compaction_started`
- `context_compaction_completed`

Each record should carry a schema version and enough identity data to be replayed independently.

`item_content_appended` is not a separate transcript item. It is an append operation against one logical item created by `item_started`. Replay folds all append operations into the same item content.

The append operation should identify the target `item_id`, content part, append offset, content kind, and appended content or content reference. Append boundaries should be chosen to balance crash recovery and storage efficiency, such as by byte threshold, time threshold, semantic boundary, or provider event boundary.

## Session Metadata

Session metadata describes current session state and references durable configuration. It is not a transcript turn.

Conceptual fields:

- `session_id`
- `workspace_root`
- `active_model_binding`
- `canonical_model_slug`
- `reasoning_effort`
- `permission_profile`
- `agent_mode`
- `persona`
- `instruction_set`
- `latest_context`
- `usage_totals`
- `last_invocation_usage`
- `active_plan`
- `active_goal`
- `created_at`
- `updated_at`

`instruction_set` includes base instructions, system instructions, active mode instructions, persona instructions, and other high-priority model-visible instruction sources. These instructions belong to metadata or context assembly state, not to user transcript turns.

When the model request is built, the request assembler may serialize metadata-derived instructions into provider-specific system or developer messages. That serialization does not make those instructions transcript turns.

## Transcript Structure

A transcript is an ordered set of turns. A turn is an execution cycle that begins with user-submitted input and ends as completed, failed, or interrupted.

Conceptual turn fields:

- `turn_id`
- `session`
- `status`
- `started_at`
- `completed_at`
- `item_refs`
- `usage_delta`
- `superseded_by_edit`
- `workspace_checkpoint_refs`
- `workspace_restore_refs`
- `interrupt_refs`
- `resume_of_turn_id`
- `resume_turn_refs`

Conceptual item fields:

- `item_id`
- `turn`
- `kind`
- `status`
- `role`
- `content_parts`
- `mentions`
- `visibility`
- `created_at`
- `completed_at`
- `revision_of`
- `superseded_by`

Item kinds include:

- `user_input`
- `assistant_text`
- `assistant_reasoning`
- `tool_call`
- `tool_result`
- `approval_request`
- `question_request`
- `steer_message`
- `queue_message`
- `error`
- `context_summary`

System instructions, base instructions, and initial metadata-derived instructions are not `user_input` items and are not their own turns.

## Tool Items

Tool call and tool result items should preserve both structured tool-domain data and natural-language summaries that help later model decisions and user review.

Conceptual tool call fields:

- `tool_call_id`
- `tool_name`
- `command_description`
- `arguments`
- `arguments_preview`
- `approval_state`
- `safety_state`
- `created_at`

Conceptual tool result fields:

- `tool_call_id`
- `status`
- `result_summary`
- `structured_status`
- `canonical_content_ref`
- `display_content_ref`
- `redaction_state`
- `completed_at`

`command_description` is the model-provided natural-language purpose for a shell or command execution tool invocation. It should be recorded with the tool call for command-like tools and should not be treated as a required field for every tool category.

`result_summary` is a factual natural-language summary derived from observed tool output or structured status. It should coexist with exact structured fields such as exit code, HTTP status, process id, or changed-file counts.

## Plan State

Plan state is user-visible task state maintained by the plan tool. It is not private model reasoning and is not merely assistant prose.

Plan records should be durable so replay can reconstruct the active plan after restart, reconnect, or session review.

Conceptual plan fields:

- `plan_id`
- `session`
- `created_turn_id`
- `updated_turn_id`
- `objective`
- `status`: active, completed, blocked, abandoned, or superseded.
- `item_refs`
- `created_at`
- `updated_at`

Conceptual plan item fields:

- `plan_item_id`
- `plan_id`
- `text`
- `status`: pending, in_progress, completed, blocked, or canceled.
- `details`
- `parent_item_id`
- `parallel_group_id`
- `source_turn_id`
- `updated_at`

Replay should project at most one active plan by default unless a later requirement explicitly allows multiple concurrent plans. Historical, superseded, completed, and abandoned plans remain auditable.

Plan state may be rendered by clients separately from transcript items. A plan update may also correspond to a tool call/result in the transcript, but the plan projection should come from durable plan records rather than parsing assistant text.

## Goal State

Goal state is durable session state for Ralph Loop autonomous work. It is not private model reasoning and is not merely assistant prose.

Replay should project at most one current goal per session in the first milestone. Historical, replaced, completed, budget-limited, canceled, and cleared goals remain auditable.

Conceptual goal fields:

- `goal_id`
- `session`
- `objective`
- `status`: active, paused, blocked, complete, budget_limited, or canceled.
- `token_budget`
- `time_budget_seconds`
- `turn_budget`
- `tokens_used`
- `time_used_seconds`
- `turns_used`
- `progress_summary`
- `blocker_summary`
- `verification_summary`
- `created_at`
- `updated_at`

Goal records should be append-only. Creating, replacing, pausing, resuming, blocking, completing, canceling, budget-limiting, clearing, or accounting usage for a goal should append a record rather than mutate earlier records.

Hidden goal context used for a model invocation should be reconstructable through `goal_context_snapshot_recorded` or through a context snapshot that references the exact hidden goal context. This hidden context is model-visible but not rendered as a normal transcript turn.

If SQLite or another indexed store is used to accelerate goal lookup, it is a projection of these durable records and must be rebuildable from the JSONL rollout.

## Interrupted And Resumed Turns

An interrupted turn remains a durable terminal turn. Partial assistant content, reasoning, tool calls, tool results, usage records, and workspace change-set records accepted before interruption must remain auditable.

Conceptual interrupt record fields:

- `interrupt_id`
- `session`
- `turn_id`
- `requested_by_client`
- `target_kind`
- `target_id`
- `interrupt_mode`
- `interrupt_status`
- `cleanup_state`
- `created_at`
- `resolved_at`

Resuming an interrupted task should create a linked continuation turn rather than mutating the interrupted turn. The continuation turn should reference the interrupted turn with `resume_of_turn_id` or equivalent provenance.

Conceptual resume record fields:

- `resume_id`
- `session`
- `resume_of_turn_id`
- `resume_turn_id`
- `resume_mode`
- `resume_content_refs`
- `context_snapshot_id`
- `created_by_client`
- `created_at`

Replay should preserve the interrupted turn and then project the resume turn as a later continuation. Active context assembly for the resumed turn may use the original user request, partial outputs, completed tool results, workspace change summaries, and user-provided resume instructions where safe.

## Active Turn Messages

`steer` and `queue` submissions are user-authored records, but they are not ordinary completed user turns when they are submitted during active work.

Conceptual durable fields:

- `message_id`
- `session`
- `active_turn`
- `submission_mode`: `steer` or `queue`.
- `content_parts`
- `mentions`
- `status`
- `created_at`
- `resolved_at`

`steer` records are associated with the active turn they are intended to influence. `queue` records preserve user-visible order until they become a later turn, are canceled, or are otherwise resolved.

Restored `steer` and `queue` records must remain distinguishable from already-executed transcript turns.

## Immediate Previous Message Edits

Editing a previous message should be represented as append-only revision data. The original user-authored message, original turn, and any original assistant/tool outputs remain durable records.

Conceptual edit record fields:

- `edit_id`
- `session`
- `target_message_id`
- `target_turn_id`
- `replacement_message_id`
- `replacement_turn_id`
- `edited_content_parts`
- `edited_mentions`
- `edit_mode`
- `created_by_client`
- `created_at`
- `edit_state`

For a completed, failed, or interrupted latest turn, replay should produce:

1. The original turn as a durable historical turn.
2. A `message_edit_recorded` event that links the original message to the replacement message.
3. Optional workspace restoration records for files changed by the superseded turn.
4. A `turn_superseded` event that marks the original turn as superseded in the current branch projection.
5. A replacement turn that uses the edited message as the current branch's user input.

For a queued message that has not started, replay should fold the accepted edit into that queue item's effective content while retaining prior queue-message revisions for audit.

For an active running turn, replay must not reinterpret already-started model or tool execution as though the edited message had been used. `session/message/edit` must interrupt the active turn and record that transition before accepting the edit; after the turn is terminal, replay follows the completed/failed/interrupted latest-turn path above.

Client projections may show the edited message as the current branch content and collapse the superseded turn by default, but audit projections must be able to recover the original message and original turn.

Active context assembly for future model invocations should use the replacement branch content. It should not include both original and edited user text as ordinary user intent unless an explicit audit or comparison task asks for that history.

## Turn Workspace Change Tracking

The program should record enough workspace change data during each turn to support later restoration if immediate message editing supersedes that turn. This change data is core-owned durable state, not client-owned rollback state.

The checkpoint should capture the workspace baseline before the turn's first mutating file operation where possible, and should record the resulting post-turn file state for changed files after each structured mutation or at turn completion. In git workspaces, an implementation may maintain hidden per-turn tree snapshots or ghost commits so the previous checkpoint can act as the pre-turn restore source.

The program should not rely on whole-workspace scanning as the primary change detection mechanism for every turn. Structured mutating tools should report exact file deltas as they complete, and the core should accumulate those deltas into a per-turn workspace change set. Broader filesystem snapshots or hidden git checkpoints may supplement this when structured deltas are insufficient, especially for shell-command changes.

Client-visible diffs are projections of the core-owned change set. They may be emitted for review, display, or progress feedback, but they are not the authoritative restore mechanism. Restoration must use the durable per-turn change set, inverse operations, content snapshots, or internal checkpoints owned by the core.

Conceptual turn checkpoint fields:

- `checkpoint_id`
- `session`
- `turn_id`
- `workspace_root`
- `checkpoint_strategy`: structured tool inverse records, hidden git checkpoint, filesystem snapshot, or unsupported.
- `baseline_ref`
- `baseline_hash`
- `created_at`
- `tool_coverage`
- `unattributed_change_policy`

Conceptual turn workspace change-set fields:

- `change_set_id`
- `session`
- `turn_id`
- `checkpoint_id`
- `structured_tool_coverage`
- `shell_change_coverage`
- `file_change_refs`
- `display_diff_ref`
- `restore_data_ref`
- `change_set_status`
- `created_at`
- `updated_at`

Conceptual per-file change fields:

- `file_change_id`
- `turn_id`
- `tool_call_id`
- `tool_name`
- `path`
- `change_kind`: create, modify, delete, rename, or mode change.
- `pre_state_ref`
- `pre_state_hash`
- `post_state_ref`
- `post_state_hash`
- `inverse_ref`
- `display_diff_hunk_ref`
- `attribution_confidence`

Known structured file-editing tools such as `write` and `apply_patch` should produce per-file change records with enough before/after state or inverse operation data to restore the pre-turn state.

Shell commands may modify files outside structured file-editing tools. Those changes should be restorable only if a turn-level workspace checkpoint, filesystem snapshot, hidden git checkpoint, or other attribution mechanism captured them. Otherwise replay should mark them as unsupported or unattributed for automatic restoration.

The program may store or emit a unified diff for a turn. A unified diff is a useful display artifact and may be sufficient for some manual review workflows, but it must not be the only durable source required for automatic restoration when richer before/after state, inverse operations, or checkpoint references are available.

Conceptual restore result fields:

- `restore_id`
- `session`
- `superseded_turn_id`
- `edit_id`
- `checkpoint_id`
- `restore_policy`
- `started_at`
- `completed_at`
- `file_results`

Conceptual per-file restore result fields:

- `path`
- `restore_status`: restored, skipped_current_state_kept, unsupported, failed, or not_needed.
- `reason`
- `expected_current_hash`
- `actual_current_hash`
- `restored_to_hash`
- `source_change_id`

Restoration should be performed by the server/core before the replacement turn begins. Clients may request an allowed restore policy and display restore outcomes, but they should not be responsible for applying inverse patches or mutating the workspace. For each file, the program should restore the pre-turn state only when the current file state still matches the expected post-turn state or another explicitly safe predicate. If the file has diverged, replay should record `skipped_current_state_kept` and the replacement turn should proceed from the current file content for that path.

A hidden git checkpoint or ghost commit can be used as an internal checkpoint strategy. It should be treated as a content-addressed workspace snapshot, not as a user-visible commit. It must not be published, staged as user work, or used to rewrite visible branch history unless the user explicitly requests that. If a git checkpoint would discard user edits made after the superseded turn, the default restore result for those files should be `skipped_current_state_kept`.

Workspace restoration affects file state only. It does not undo external API effects, running process effects, network calls, published git commits, or other non-file side effects. Those effects remain auditable in the superseded turn.

## Mentions

User input items should include a dedicated `mentions` field. A mention records structured references detected or selected inside a user message, separate from the user-visible content text.

Mention examples:

- Skill reference.
- File or directory reference.
- MCP server, resource, or template reference.
- Tool or connector reference.
- Session, turn, or transcript reference.
- Image, pasted artifact, or attachment reference.

Conceptual mention fields:

- `mention_id`
- `kind`
- `display_text`
- `target`
- `source_range`
- `resolution_status`
- `visibility`

`content_parts` represent what the user submitted or what the assistant/tool produced. `mentions` represent structured references extracted from or attached to that content. A pasted image may therefore appear as a multimodal content part and also have a mention record that tracks how the program resolved or referenced that artifact.

## Content Parts

Items should support multimodal content parts.

Conceptual content part kinds:

- `text`
- `image_ref`
- `file_ref`
- `audio_ref`
- `video_ref`
- `tool_call_json`
- `tool_result_text`
- `provider_metadata`

Large binary artifacts should be stored outside inline JSONL content and referenced by stable artifact references.

## Fork Origin And Inherited History

A forked session should store durable origin metadata and a replayable inherited-history segment rather than copying the full parent transcript into every fork.

The origin metadata explains where the fork came from. It is not the only content pointer for replay, because a parent session may later be deleted or unavailable.

Conceptual fork origin fields:

- `parent_session_id`
- `fork_turn_id`
- `fork_created_at`
- `parent_display_label`
- `fork_turn_display_label`
- `fork_turn_digest`
- `origin_snapshot_hash`
- `parent_availability`

Fork origin fields preserve provenance and user-facing labels. They are not guaranteed to remain dereferenceable links after parent deletion.

Conceptual inherited-history segment fields:

- `inherited_segment_id`
- `source_parent_session_id`
- `source_range`
- `storage_strategy`: protected shared segment, materialized fork segment, or protected retained source records.
- `record_refs` or `materialized_record_refs`
- `segment_hash`
- `availability_state`

The inherited-history segment describes the parent transcript range that is visible and usable in the fork. A fork replay must be able to reconstruct that inherited transcript from the inherited segment without requiring the deleted parent session file to be opened.

`source_parent_session_id` and `source_range` identify where the segment came from, but they are provenance keys after parent deletion. They must not be treated as the only way to load the inherited content.

Parent deletion rules:

- Deleting a parent session must not make surviving forked sessions unusable.
- If forked sessions still reference inherited source records, those records must remain in a protected shared segment, be materialized into the fork, or be protected by another explicit retention mechanism before the parent session is made inaccessible.
- A surviving fork must not rely only on `parent_session_id` plus `fork_turn_id` to recover inherited content after parent deletion.
- Deleting the parent session may make the parent session index entry and parent event file inaccessible, but it must not remove any inherited-history segment still required by a surviving fork.
- A forked session whose parent has been deleted should retain a fork indicator with `parent_availability` set to deleted or unavailable.
- Navigation to a deleted parent may fail, but inherited history visible in the fork must remain understandable.
- Hard purge of parent records referenced by surviving forks must be blocked unless the inherited segment is first materialized or moved to protected shared storage, or unless the user explicitly chooses cascade deletion of dependent forks where supported.

## Internal Persistent Memory Links

Persistent memory is stored outside the session transcript and is maintained by the core agent runtime. Durable session records may include internal provenance links where useful for replay, debugging, safety, or context-quality analysis.

These links are not client-managed transcript items and are not part of the routine client projection.

Conceptual memory link fields:

- `memory_id`
- `source_session_id`
- `source_turn_id`
- `source_item_id`
- `derivation_event`
- `source_availability`

When a session is deleted, the core may update, unlink, retain, or remove internal memory links according to internal memory policy. Session replay must not require clients to make per-memory decisions, and ordinary client projections should not expose memory-link records.

## Active Context

The active context object is distinct from the full transcript. It should reference transcript items, item ranges, summaries, instruction-set records, and artifact records rather than duplicating their full content.

Conceptual context snapshot fields:

- `context_id`
- `session`
- `created_for_turn`
- `model_binding`
- `instruction_set_ref`
- `entries`
- `token_estimate`
- `immutable_prefix_hash`
- `created_at`

Conceptual context entry kinds:

- `instruction_ref`
- `transcript_item_ref`
- `transcript_range_ref`
- `context_summary_ref`
- `artifact_ref`

When context approaches the model's effective context limit, compaction creates a summary item or summary record and a later context snapshot references that summary instead of all older detailed transcript records. The full transcript remains available for user review.

Compaction lifecycle records should preserve whether compaction was triggered manually by the user or automatically by context pressure. Clients use that trigger source to render transcript-area status cells with `Manual Compaction Started`, `Automatically Compaction Started`, and `Compaction Done` while keeping the compaction summary itself as context state rather than ordinary user or assistant transcript text.

## Token Usage

Token usage should be recorded as per-invocation deltas and replayed into session totals.

Conceptual usage fields:

- `input_tokens`
- `output_tokens`
- `cache_creation_input_tokens`
- `cache_read_input_tokens`
- `reasoning_output_tokens`
- `total_tokens`

These are the canonical provider/model usage fields for durable turn usage and session replay. `output_tokens` is the provider-mapped primary output token count. `reasoning_output_tokens` is an optional breakdown when a provider reports it, and replay must never add it separately to `output_tokens` or `total_tokens`.

`total_tokens` is present only when a provider reported a total or when a persisted aggregate stores the display total. For a single usage delta, display total selection is:

```text
display_total_tokens = total_tokens.unwrap_or(input_tokens + output_tokens)
```

Session `total_tokens` is the sum of each invocation's display total. This preserves provider totals when they are available and avoids double-counting reasoning tokens when no provider total exists.

For context pressure, the active context token estimate should be tracked separately from billing or response totals.

## Streaming Persistence

Streaming responses should be persisted incrementally.

Conceptual streaming sequence:

```text
turn_started
item_started
item_content_appended
item_content_appended
item_completed
usage_recorded
turn_completed
```

If the program exits unexpectedly, replay can recover completed content append operations and mark the incomplete turn or item as interrupted.

The durable storage layer should not write one JSONL record for every provider SSE delta. It should buffer and append coalesced content operations often enough to preserve useful partial output after a crash while avoiding unbounded storage overhead.

This design keeps a single logical response item. Multiple append records are storage operations used to rebuild the item after replay; they are not multiple response objects in the transcript or client projection.

## Metadata Changes And Model Visibility

Because JSONL is append-only, session metadata changes are represented by appended metadata events or snapshots rather than in-place mutation.

Before each LLM invocation, the program should:

1. Replay or load the current metadata state.
2. Detect metadata changes relevant to the next invocation.
3. Build active context from metadata and transcript references.
4. Serialize only the model-visible subset into provider request messages.

Not every metadata change must become model-visible text. For example, token totals are runtime metadata. Persona, active mode, permission posture, workspace, and instruction-set changes may need model-visible representation depending on the current requirements.

## Reference Representation Note

This design uses ID-shaped fields such as `session_id`, `turn_id`, `item_id`, and `context_id` to make durable references explicit in diagrams and JSONL records.

Implementation data structures do not need to store every relationship as a raw ID string after records are loaded. Server runtime code may hold resolved references, owned structs, indexes, arenas, handles, or other idiomatic structures. The requirement is that durable JSONL records preserve stable references and that runtime structures can be projected back to the durable reference model without ambiguity.

## Server And Client Projections

The server owns replay, mutation, context assembly, provider request construction, and persistence.

Client projections should receive only the data needed for display and interaction, such as:

- Session summary.
- Turn status.
- Visible transcript items.
- Token usage summary.
- Current model and reasoning display.
- Active plan state.
- Active goal state.
- Plan Mode or permission state display.
- Mention display and resolution status.

Clients do not need full active-context internals unless they are explicitly showing context diagnostics.

Clients also do not need persistent-memory internals. Persistent memory may affect model-visible context assembled by the core, but routine client projections should not expose memory records, memory-link records, or memory-change events.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-CONV-001 | 1 | specs/L1/L1-REQ-CONV-001-session-lifecycle.md | Defines the durable session data model used for lifecycle persistence. |
| refines | L1-REQ-CONV-002 | 1 | specs/L1/L1-REQ-CONV-002-turn-lifecycle.md | Defines turn and item data structures for auditable turns. |
| related-to | L1-REQ-AGENT-001 | 1 | specs/L1/L1-REQ-AGENT-001-execution-workflow.md | Durable records preserve execution workflow state. |
| related-to | L1-REQ-AGENT-002 | 1 | specs/L1/L1-REQ-AGENT-002-interrupt-resume.md | Durable records preserve interrupted and resumed turn state. |
| related-to | L1-REQ-AGENT-003 | 1 | specs/L1/L1-REQ-AGENT-003-task-planning.md | Durable plan records preserve visible task planning state. |
| related-to | L1-REQ-CONV-003 | 1 | specs/L1/L1-REQ-CONV-003-active-turn-message-handling.md | Defines durable steer and queue records. |
| refines | L1-REQ-CONV-004 | 1 | specs/L1/L1-REQ-CONV-004-session-forking.md | Defines fork references and deletion retention behavior. |
| refines | L1-REQ-CONV-005 | 1 | specs/L1/L1-REQ-CONV-005-immediate-message-editing.md | Defines append-only message edit records and replacement turn references. |
| related-to | L1-REQ-CHANGE-001 | 1 | specs/L1/L1-REQ-CHANGE-001-rollback-and-recovery.md | Defines core-owned workspace change-set and restoration records for superseded turns. |
| related-to | L1-REQ-EDIT-001 | 1 | specs/L1/L1-REQ-EDIT-001-file-editing-workflow.md | Structured tool file changes provide restoration inputs. |
| related-to | L1-REQ-GOAL-001 | 1 | specs/L1/L1-REQ-GOAL-001-ralph-loop.md | Durable goal records preserve objective, status, budget, progress, and hidden goal context snapshots for replay. |
| related-to | L1-REQ-GIT-001 | 1 | specs/L1/L1-REQ-GIT-001-change-management.md | Hidden git checkpoints may support turn restoration. |
| related-to | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | Durable records preserve built-in tool calls, results, and plan state. |
| related-to | L1-REQ-APP-002 | 1 | specs/L1/L1-REQ-APP-002-persistence.md | Defines JSONL replay and recovery records for durable conversation history. |
| related-to | L1-REQ-APP-012 | 1 | specs/L1/L1-REQ-APP-012-privacy-data-ownership.md | Defines privacy handling for model-visible user data. |
| related-to | L1-REQ-MEM-001 | 1 | specs/L1/L1-REQ-MEM-001-persistent-memory.md | Defines persistent memory as core-maintained internal state. |
| related-to | L1-REQ-CONTEXT-001 | 1 | specs/L1/L1-REQ-CONTEXT-001-management.md | Defines active context as references into transcript and metadata. |
| related-to | L1-REQ-CONTEXT-003 | 1 | specs/L1/L1-REQ-CONTEXT-003-compress.md | Defines compaction output as durable summary records referenced by active context. |
| related-to | L1-REQ-INPUT-001 | 1 | specs/L1/L1-REQ-INPUT-001-attachments-and-multimodal.md | Defines content parts and mentions for attachments and multimodal input. |
| related-to | L1-REQ-TUI-003 | 1 | specs/L1/L1-REQ-TUI-003-transcript.md | Defines transcript structures that clients render. |
| related-to | L1-REQ-LLM-001 | 1 | specs/L1/L1-REQ-LLM-001-token-efficiency.md | Preserves immutable context prefixes through append-only storage and context references. |
| related-to | L2-DES-MODEL-001 | 2 | specs/L2/model/L2-DES-MODEL-001-model-provider-binding.md | Session metadata references configured model bindings. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | Defines tool and plan records persisted by the session data model. |
| related-to | L2-DES-GOAL-001 | 1 | specs/L2/goal/L2-DES-GOAL-001-ralph-loop-goals.md | Defines the goal records and projection semantics persisted by the session data model. |
| specified-by | L3-BEH-CORE-001 | 6 | specs/L3/core/L3-BEH-CORE-001-session-jsonl-data-model.md | L3 defines durable JSONL records, append timing, replay, and crash recovery. |
| specified-by | L3-BEH-CORE-006 | 3 | specs/L3/core/L3-BEH-CORE-006-turn-item-data-structures.md | L3 defines turn, item, content part, mention, plan, goal, and file-change structures. |
| specified-by | L3-BEH-CORE-011 | 1 | specs/L3/core/L3-BEH-CORE-011-session-forking-retention.md | L3 defines session fork admission, inherited-history segment creation, replay, and parent-deletion retention behavior. |
| specified-by | L3-BEH-CORE-012 | 1 | specs/L3/core/L3-BEH-CORE-012-message-edit-workspace-restore.md | L3 defines immediate previous message editing, workspace restoration, turn supersession, branch projection, and restore replay behavior. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial session JSONL data model with metadata, transcript, mentions, active context references, streaming persistence, and reference representation note. |
| 1 | 2026-05-22 | Human | Refinement | Split provider, server-client, and durable event planes and replaced durable item deltas with coalesced content append operations. |
| 1 | 2026-05-22 | Human | Refinement | Clarified that durable content append records are operations on one logical item, not multiple item objects. |
| 1 | 2026-05-22 | Human | Refinement | Added steer and queue records, fork deletion retention behavior, and persistent memory provenance links. |
| 1 | 2026-05-22 | Human | Refinement | Added append-only immediate previous message edit records and replacement turn projection behavior. |
| 1 | 2026-05-22 | Human | Refinement | Clarified that fork origin keys may become non-dereferenceable after parent deletion and cannot be the fork's only inherited content pointer. |
| 1 | 2026-05-22 | Human | Refinement | Added turn workspace checkpoints and restore result records for superseded-turn file restoration. |
| 1 | 2026-05-22 | Human | Refinement | Reframed persistent memory links as internal core provenance outside routine client projections. |
| 1 | 2026-05-22 | Human | Refinement | Clarified that per-turn workspace change sets are core-owned restore data, while client-visible diffs are display projections. |
| 1 | 2026-05-22 | Human | Refinement | Added durable interrupt and resume records for execution engine recovery. |
| 1 | 2026-05-22 | Human | Refinement | Added durable plan records for plan-tool state and replay. |
| 1 | 2026-05-22 | Human | Refinement | Added durable command descriptions and natural-language result summaries. |
| 1 | 2026-05-23 | Human | Refinement | Added durable Ralph Loop goal records and active-goal projection fields. |
| 1 | 2026-05-25 | Human | Refinement | Added manual versus automatic compaction trigger provenance for transcript-area compaction notices. |
| 1 | 2026-06-23 | Assistant | Refinement | Added conservative usage fields, provider-total fallback semantics, and reasoning breakdown double-count protection. |
