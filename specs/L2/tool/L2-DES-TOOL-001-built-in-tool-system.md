---
artifact_id: L2-DES-TOOL-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-09-07
---

# L2-DES-TOOL-001 — Built-In Tool System

## Purpose

Define the built-in tool system used by the agent execution engine, including baseline tool categories, tool lifecycle, safety gates, visibility, and the plan tool that maintains user-visible task planning state.

## Background / Context

The agent execution engine depends on tools to inspect the workspace, modify files, run commands, ask for approval, ask Plan Mode clarification questions, search, fetch external content, coordinate subagents, and keep a visible plan.

`L2-DES-AGENT-001` defines where model-requested tool dispatch occurs in the execution loop. This design defines the tool system that dispatch uses.

The plan tool is part of the tool system rather than private model reasoning. It updates a user-visible to-do list that represents planned work and execution status.

## Source Requirements

- `L1-REQ-TOOL-002` requires a baseline set of built-in tools.
- `L1-REQ-AGENT-003` requires visible task planning with status updates.
- `L1-REQ-AGENT-005` restricts the question tool to Plan Mode.
- `L1-REQ-LLM-002` requires model-requested tool use through a controlled lifecycle.
- `L1-REQ-TOOL-001` requires tool safety, approval, redaction, and bounded output.
- `L1-REQ-APP-003` requires permission modes, sandboxing, and explicit user approval for actions outside the current permission boundary.
- `L1-REQ-TOOL-005` requires background process visibility and manual stop behavior.
- `L1-REQ-TOOL-003` requires configurable web search behavior.
- `L1-REQ-TOOL-004` requires explicit parallel tool orchestration where enabled.
- `L1-REQ-AGENT-004` requires subagent delegation where enabled.
- `L1-REQ-GOAL-001` requires verified completion and blocker reporting for Ralph Loop goals.
- `L1-REQ-APP-010` requires configuration and unavailable-state behavior.
- `L2-DES-AGENT-001` defines the execution engine that dispatches tools.
- `L2-DES-AGENT-002` defines interruption and background process control.
- `L2-DES-APP-003` defines protocol events that expose tool and plan state.
- `L2-DES-CONV-001` defines durable tool, plan, and transcript records.
- `L2-DES-GOAL-001` defines the narrow model-facing goal update tool.
- `L2-DES-TOOL-002` defines explicit `multi_tool_use` parallel orchestration behavior.

## Design Requirement

The program should provide a server-owned tool registry and tool supervisor. The model may request tools through structured tool calls, but the server validates, authorizes, executes, records, and reports every tool call.

Tool calls are not arbitrary code paths. Each tool must be defined by metadata, schema, capability classification, safety policy, output policy, and runtime handler.

## Tool Registry

Conceptual `ToolDefinition` fields:

- `tool_name`
- `display_name`
- `description`
- `input_schema`
- `output_schema`
- `tool_category`
- `execution_mode`: read_only, mutating, command, background_process, user_prompt, planning, goal_status, delegation, web, or internal.
- `availability`: available, disabled, needs_configuration, unsupported, or blocked_by_mode.
- `configuration_refs`
- `permission_profile`
- `permission_policy`
- `redaction_policy`
- `output_limit_policy`
- `supports_streaming_output`
- `supports_cancellation`
- `supports_parallel_execution`

The registry should expose only tools available to the current session mode, permission posture, configuration, and model capability. Disabled tools should be omitted from the model-facing tool list. A stale or misconfigured tool call should fail explicitly with a structured unavailable result rather than fabricating output.

## Permission Policy And Sandbox

Tool execution is governed by permission policy and sandbox policy as separate layers.

This design is one implementation surface for `L1-REQ-APP-003`: tool permission policy decides whether an action can proceed, needs review, or needs user approval, while sandbox policy constrains host access even after approval.

`permission_policy` controls whether a tool call may proceed automatically, requires review, or requires user approval. The initial policy values are:

- `default`: baseline permission behavior for normal sessions.
- `auto_review`: review-oriented behavior that classifies tool calls before execution and requires user approval when risk or ambiguity remains.
- `full_access`: broad permission behavior for trusted contexts, while still preserving validation, mode constraints, privacy rules, audit recording, and sandbox restrictions.

Sandbox policy controls what the tool execution process can do at the host boundary. The durable sandbox configuration schema is not finalized, but the sandbox design target is to restrict system calls such as `open`, `read`, and `write`, with practical controls over:

- Directory read access.
- Directory write access.
- File creation, mutation, rename, and deletion.
- Process execution boundaries where supported.
- Network access at the domain level.

The permission policy must not be treated as the sandbox. `full_access` can reduce approval prompts only inside the limits still imposed by the sandbox and tool validation.

## Command Intent Inputs

Shell or command execution tools should include an invocation-level `description` field at the beginning of their input schema. The field should be a concise natural-language sentence describing what the model intends the command to accomplish before the model provides the command text.

This requirement is specific to command-like tools. Structured tools such as file read, file write, apply patch, search, approval, question, plan, and subagent coordination should not be forced to add a generic `description` field when their schema already carries intent through typed arguments.

Conceptual command tool invocation fields:

- `description`
- `command`
- `timeout` where applicable.
- `working_directory` where applicable.
- `sandbox_permissions` (`use_default`, `with_additional_permissions`, or `require_escalated`) where the tool spawns a process.
- `additional_permissions` when requesting a sandboxed overlay.
- `justification` and optional `prefix_rule` when requesting full escalation.
- `tool_call_id`
- `session_id`
- `turn_id`
- `requested_at`

The command `description` field is not hidden reasoning and not a substitute for validation. It is an explicit intent summary used by the runtime, audit trail, and model-facing context. It helps bind a shell command to a natural-language purpose before the command text is generated and executed.

The command text may still include normal shell comments, including a first-line intent comment, when that is useful for command readability or model generation quality. Such comments are part of the executable script text sent to the shell. The runtime should not parse shell comments as protocol metadata, and it should not require a command comment to duplicate the structured `description` field.

Command execution tools should use an input shape where `description` precedes `command`. For example:

- `description`: one sentence describing the command's intended outcome.
- `command`: the command text to execute.

Example command-tool input:

```json
{
  "description": "Check if the dev server is running, then capture any errors.",
  "command": "# Check if the dev server is running, then capture any errors.\nsleep 5\nSERVER_STATUS=$(curl -s -o /dev/null -w \"%{http_code}\" http://localhost:3000)\nif [ \"$SERVER_STATUS\" = \"200\" ]; then\n  echo \"Dev server is healthy, checking for runtime errors\"\n  cat /tmp/app.log | grep -i \"error\" | tail -20\nelse\n  echo \"Dev server failed to start (HTTP $SERVER_STATUS), showing startup logs\"\n  cat /tmp/app.log | tail -50\nfi"
}
```

In this example, the command description states the canonical intent before the command body. The command body also includes a normal shell comment and translates raw HTTP status into natural-language status lines that are easier for the model and user to interpret.

The server should validate the actual command independently from the description. If the description and command conflict, the server should treat the call as suspicious, invalid, or approval-worthy according to safety policy.

## Baseline Tool Categories

The baseline built-in tool set should cover these categories:

| Category | Purpose | Examples |
|---|---|---|
| File read | Inspect file contents and metadata. | read file, list directory. |
| File mutation | Create, edit, delete, or rename files through structured operations. | write, apply patch. |
| Search | Find files, literal content, or semantically relevant code in the workspace. | file-name search, content search, semantic code retrieval. |
| Command execution | Run shell commands with bounded output. | one-shot command execution. |
| Background process | Track long-running commands and process stdin. | dev server, test watcher, interactive command. |
| Planning | Maintain visible task plan state. | plan tool. |
| Goal status | Report verified goal completion or blockers. | goal update tool. |
| Approval | Ask the user for permission before risky actions. | approval request. |
| Question | Ask the user for clarification in Plan Mode only. | question tool. |
| Web | Fetch or search external content where configured. | web fetch, web search. |
| Delegation | Start or coordinate subagents where enabled. | subagent spawn/status/result. |
| Parallel orchestration | Execute an explicit group of valid tool calls concurrently. | `multi_tool_use`. |

Exact tool names and schemas are L3 concerns. This L2 design defines the categories and lifecycle constraints.

## Semantic Code Retrieval

The built-in search category may include a read-only semantic code retrieval tool for agent codebase navigation. This tool complements literal content search rather than replacing it.

The semantic retrieval tool should:

- Accept a search operation for natural-language or code-symbol queries.
- Accept a related-code operation that starts from a workspace file path and 1-indexed line number.
- Restrict all roots and source paths to the active workspace boundary.
- Support content filters for code, docs, config, and all indexed non-data text.
- Return bounded structured JSON results containing score, workspace-relative file path, line range, language, and chunk content.
- Use workspace search permissions and remain eligible for parallel read-only execution.
- Chunk supported programming and structured config languages with tree-sitter AST boundaries, while using line-based fallback for docs, data, unsupported language labels, parser errors, or empty AST output.
- Cache indexes locally when possible, while invalidating them when the indexed file manifest, content mode, or embedding model changes.
- Refresh cached indexes incrementally when possible, re-embedding changed files while reusing unchanged file records, and use watcher-backed warm reuse only when the watched root is clean and within a bounded safety interval.
- Warm the default code-only index on a dedicated background thread when a root session establishes its authoritative workspace. Session readiness must not wait for indexing, concurrent searches must share the in-flight build, and warmup failure must remain retryable by the first foreground search.
- Store local vector cache data in compact binary form where practical, and use approximate nearest-neighbor candidate search plus exact reranking for large semantic indexes while preserving exact fallbacks for filtered or small searches.
- Return a structured unavailable result when the embedding model cannot be loaded or cached, rather than fabricating semantic results.

The initial retrieval design uses hybrid ranking: dense code embeddings, BM25 sparse retrieval over code-aware identifier tokens, reciprocal-rank fusion, and code-aware reranking that can boost symbol definitions and path matches while de-emphasizing tests, examples, docs, compatibility, and legacy paths.

## Tool Invocation Lifecycle

The normal lifecycle for a model-requested tool call is:

1. Provider stream emits a structured tool request, including a command intent `description` for shell or command execution tools.
2. Execution engine normalizes the request into an internal tool invocation.
3. Tool registry resolves the tool definition.
4. Input is validated against the tool schema.
5. Mode, permission, safety, and configuration gates are evaluated.
6. Approval is requested if required.
7. The tool supervisor executes the handler or returns a structured denial/unavailable result.
8. Streaming progress is emitted where supported.
9. Output is bounded, redacted, and normalized.
10. Durable tool call and tool result records are appended.
11. Server-client events update subscribed clients.
12. The structured result is returned to the model if the provider interaction continues.

Every tool invocation should produce one of these terminal states:

- `completed`
- `denied`
- `blocked_by_mode`
- `needs_configuration`
- `invalid_input`
- `failed`
- `canceled`
- `interrupted`

## Plan Tool

The plan tool maintains a visible to-do list for the current session or active task. It is the primary mechanism for satisfying visible task-planning requirements without exposing private model reasoning.

The plan tool should be available in Normal Mode and Plan Mode. In Plan Mode it may be used to build a strategic plan without mutating files. In Normal Mode it may be used to track execution progress.

Conceptual plan fields:

- `plan_id`
- `session_id`
- `turn_id` where created or last updated.
- `objective`
- `items`
- `status`: active, completed, blocked, abandoned, or superseded.
- `created_by`: agent, user, or imported.
- `created_at`
- `updated_at`

Conceptual plan item fields:

- `plan_item_id`
- `text`
- `status`: pending, in_progress, completed, blocked, or canceled.
- `details`
- `parent_item_id`
- `parallel_group_id`
- `source_turn_id`
- `updated_at`

Plan tool operations should include:

- Create or replace an active plan.
- Add plan items.
- Update item status.
- Mark the overall plan complete, blocked, abandoned, or superseded.
- Attach a blocker or short status note.
- Represent explicit parallel work through multiple in-progress items or a `parallel_group_id`.

The plan tool output is program state, not hidden chain-of-thought. Plan item text should be concise, user-visible, and action-oriented.

## Plan Consistency

The execution engine should keep plan state consistent with actual execution state:

- When a planned step starts, the corresponding item should become `in_progress`.
- When a planned step finishes, the corresponding item should become `completed`.
- When execution cannot continue, the corresponding item should become `blocked` with a concise reason.
- If the user's objective changes, the active plan should be updated, superseded, or explicitly abandoned.
- If work is delegated to subagents, plan state should identify delegated or parallel work.

The plan tool should not be mandatory for trivial one-step tasks. The agent may create a plan when task complexity, risk, user request, Plan Mode, or parallel work justifies it.

## Goal Update Tool

The goal update tool lets the model report that the current Ralph Loop goal is verified complete or blocked. It is not the user-facing `/goal` command and must not expose user-owned goal controls to the model.

Allowed operations:

- Mark the current goal `complete` with a verification summary and evidence references.
- Mark the current goal `blocked` with a blocker summary and the user input or external state change needed to continue.

Disallowed operations:

- Create a goal.
- Replace a goal.
- Edit the objective.
- Increase, reset, or remove a budget.
- Pause, resume, clear, or cancel a goal.

The tool should include `expected_goal_id` so stale model output cannot update a replaced goal. If the goal changed after context assembly, the tool result should become a no-op with a factual stale-state summary.

The tool result is program state. It should produce durable goal records and client-visible goal events as defined by `L2-DES-GOAL-001`, `L2-DES-CONV-001`, and `L2-DES-APP-003`.

## Mode Gating

Tools must respect session-local collaboration mode and session-level agent mode.

Plan Mode constraints:

- Mutating file tools must be blocked.
- The question tool may be available for clarification.
- Read-only tools, search tools, and safe inspection tools may be available.
- The plan tool should be available.
- Command, web, and subagent tools should follow explicit mode policy because they may have non-file side effects.

Normal Mode constraints:

- The question tool must be blocked unless a later requirement explicitly allows another mode.
- The plan tool remains available for visible progress tracking.
- Mutating tools may be available subject to permission and safety policy.

Tool availability should be resolved before tool schemas are exposed to the model where practical. If a provider still emits a blocked tool call, the server must return a structured blocked result.

## Parallel Tool Orchestration

`multi_tool_use` is an explicit orchestration tool. Detailed behavior is refined by `L2-DES-TOOL-002`. This document keeps only the shared tool-system invariants.

Rules:

- Each child tool call must be independently resolved, validated, authorized, and recorded.
- Child calls are scheduled concurrently by the orchestration tool.
- The parent orchestration tool must not serialize, reorder, reject, or downgrade the group solely because of an additional runtime parallel-safety classification.
- Each child still follows the same validation, permission, approval, sandbox, availability, and safety behavior that would apply if invoked directly.
- The parent orchestration result should preserve child ordering, child ids, terminal states, and partial failures.
- Progress events should be emitted per child call so clients can render work before the entire group completes.

## Output And Redaction

Tool output should be split into:

- Natural-language status summary returned to the model.
- Canonical result content returned to the model.
- Display content safe for clients.
- Structured status fields such as exit code, HTTP status, process id, or file counts.
- Durable output references where output is large.
- Redaction metadata explaining whether secrets or unsafe content were removed.

The natural-language status summary should translate raw tool-domain signals into a concise statement that supports the next agent decision. For example, a command result should not expose only `exit_code: 1`; it should also provide a summary such as "Command failed: port 3000 is already in use by process 8432." Structured fields should still be retained for exactness, replay, and UI display.

Tool result summaries should be factual and derived from observed tool output or structured status. They must not invent likely causes or next actions. When the tool can identify a cause, the summary should name it directly; when it cannot, it should say what is known.

Output must be bounded so noisy commands, large files, web content, or background processes do not make the client or model context unusable.

## Background Processes

Command tools may start processes that continue after the originating tool call returns. Such processes should be registered with the tool supervisor and exposed through `L2-DES-AGENT-002` and `L2-DES-APP-003`.

`exec_command.execution_mode` accepts `attached` (default) or `background`. Attached execution returns a short-lived `process_id` for subsequent `write_stdin` calls; this process handle is distinct from the conversation `session_id`. `write_stdin` continues to accept the legacy `session_id` input for compatibility, but new tool definitions advertise only `process_id`. Background execution returns a `TaskId` immediately and registers a command task that is visible through the shared `list_tasks`, `await_task`, and `cancel_task` tools. Command tasks use the same public states as agent tasks: `waiting_approval`, `running`, `completed`, `failed`, and `canceled`.

`await_task` returns background command output only after terminal completion; a deadline expiry returns task metadata without partial command output. `cancel_task` terminates the process, removes it from the live process store, and retains canceled task metadata for subsequent inspection.

The built-in tool system should record:

- Process id where available.
- Command label.
- Session and turn association.
- Workspace root.
- Runtime status.
- Recent output reference.
- Stdin capability.
- Stop capability.

Detailed process termination semantics are refined by interrupt/resume and future tool L3 designs.

## Durable Recording

The tool system should produce durable records through `L2-DES-CONV-001` for:

- Tool calls.
- Tool results.
- Tool progress summaries where needed for replay.
- Plan creation and updates.
- Goal completion or blocker reports.
- Approval and question requests.
- Background process registration and terminal state.
- Workspace change-set records from structured mutating tools.

Live server-client events may be more frequent than durable records, but replay must recover the final visible tool and plan state.

## Invariants

- Tools execute only through the server-owned tool supervisor.
- Tool schemas exposed to the model reflect current mode, configuration, and capability state where practical.
- Shell or command execution tool calls include a concise intent `description` before the command text.
- Tool calls cannot bypass validation, safety, approval, or mode gates.
- The question tool is blocked in Normal Mode.
- The plan tool creates visible plan state and does not expose private model reasoning.
- The goal update tool can only report verified completion or blockers; it cannot modify user-owned goal parameters.
- Mutating tools report file changes to the core-owned workspace change set where supported.
- Tool outputs are bounded and redacted before model or client exposure.
- Tool outputs include natural-language status summaries alongside structured status fields.
- A stale or misconfigured tool call returns a clear unavailable result; a disabled tool is omitted from the model-facing tool list.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | Defines the built-in tool registry, categories, lifecycle, and baseline tool behavior. |
| refines | L1-REQ-AGENT-003 | 1 | specs/L1/L1-REQ-AGENT-003-task-planning.md | Defines the plan tool as visible to-do state for task planning and progress. |
| related-to | L1-REQ-AGENT-005 | 1 | specs/L1/L1-REQ-AGENT-005-plan-mode.md | Applies Plan Mode restrictions to mutating tools and question-tool availability. |
| related-to | L1-REQ-LLM-002 | 1 | specs/L1/L1-REQ-LLM-002-tools.md | Defines the controlled lifecycle for model-requested tools. |
| related-to | L1-REQ-TOOL-001 | 1 | specs/L1/L1-REQ-TOOL-001-safety.md | Tool safety, approval, and redaction gates apply to all tool calls. |
| related-to | L1-REQ-APP-003 | 1 | specs/L1/L1-REQ-APP-003-safety.md | Tool execution remains bounded by permissions, sandboxing, and user approval. |
| related-to | L1-REQ-TOOL-003 | 1 | specs/L1/L1-REQ-TOOL-003-web-search-configuration.md | Web search is a configurable built-in tool category. |
| related-to | L1-REQ-TOOL-004 | 1 | specs/L1/L1-REQ-TOOL-004-parallel-tool-orchestration.md | `multi_tool_use` is the explicit parallel orchestration tool. |
| related-to | L1-REQ-TOOL-005 | 1 | specs/L1/L1-REQ-TOOL-005-background-process-management.md | Command tools can register background processes for inspection and stop control. |
| related-to | L1-REQ-AGENT-004 | 1 | specs/L1/L1-REQ-AGENT-004-subagents.md | Subagent coordination is a built-in delegation tool category where enabled. |
| related-to | L1-REQ-GOAL-001 | 1 | specs/L1/L1-REQ-GOAL-001-ralph-loop.md | Defines the narrow model-facing goal update tool for verified completion and blockers. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | The execution engine dispatches tools through this tool system. |
| related-to | L2-DES-AGENT-002 | 1 | specs/L2/agent/L2-DES-AGENT-002-interrupt-resume-control.md | Interrupt and resume control active tool and background process work. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Protocol events expose tool and plan state to clients. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Durable records preserve tool and plan state. |
| related-to | L2-DES-GOAL-001 | 1 | specs/L2/goal/L2-DES-GOAL-001-ralph-loop-goals.md | Defines goal status transitions exposed through the goal update tool. |
| related-to | L2-DES-TOOL-002 | 1 | specs/L2/tool/L2-DES-TOOL-002-parallel-tool-orchestration.md | Refines explicit `multi_tool_use` child scheduling, visibility, aggregation, and cancellation. |
| specified-by | L3-BEH-TOOLS-001 | 2 | specs/L3/tools/L3-BEH-TOOLS-001-tool-contracts.md | L3 defines pure tool contracts and registry traits. |
| specified-by | L3-BEH-CORE-003 | 2 | specs/L3/core/L3-BEH-CORE-003-tool-handlers.md | L3 defines built-in handler implementations and registry construction. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-22 | Assistant | Initial | Initial built-in tool system and plan tool design. |
| 1 | 2026-05-22 | Human | Refinement | Added command intent inputs and natural-language tool status summaries. |
| 1 | 2026-05-23 | Human | Refinement | Added the narrow model-facing goal update tool for Ralph Loop completion and blockers. |
| 1 | 2026-05-25 | Human | Refinement | Renamed tool metadata from `approval_policy` to `permission_policy` and separated permission policy from sandbox enforcement direction. |
| 1 | 2026-05-25 | Assistant | Refinement | Added `L1-REQ-APP-003` as the application-safety source for tool permission, sandbox, and approval behavior. |
| 1 | 2026-05-26 | Assistant | Refinement | Linked `multi_tool_use` behavior to `L2-DES-TOOL-002` and removed runtime-selected serialization wording. |
| 1 | 2026-06-04 | Assistant | Refinement | Added read-only semantic code retrieval as a workspace search capability with structured output, model-unavailable behavior, and cache invalidation expectations. |
| 1 | 2026-06-04 | Assistant | Refinement | Clarified semantic code retrieval cache refresh as incremental per-file re-embedding with watcher-backed warm reuse bounded by a safety interval. |
| 1 | 2026-06-04 | Assistant | Refinement | Clarified semantic code retrieval performance internals for binary vector cache storage, ANN candidate search with exact reranking, and exact fallbacks. |
| 1 | 2026-06-04 | Assistant | Refinement | Clarified semantic code retrieval chunking as tree-sitter AST chunking for supported programming/config languages with line fallback for docs, data, unsupported labels, and parser failures. |
| 1 | 2026-09-07 | Assistant | Refinement | Command tools that spawn a process advertise sandbox escalation fields (`sandbox_permissions`, overlay, justification, `prefix_rule`). |
