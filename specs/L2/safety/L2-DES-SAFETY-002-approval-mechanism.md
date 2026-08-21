---
artifact_id: L2-DES-SAFETY-002
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-SAFETY-002 — Approval Mechanism Design

## Purpose

Define the approval mechanism that governs when and how the agent must obtain user consent before executing sensitive operations. This document covers approval policy levels, the tiered approval flow, escalation paths, approval scoping, caching, auto-review, and the TUI contract.

## Scope

This document covers:
- Approval policy levels and their semantics
- The layered approval decision flow
- Escalation model (additional permissions vs full escalation)
- Approval scope types and caching
- Auto-reviewer design
- TUI approval overlay contract
- Per-tool approval overrides
- Approval lifecycle events

This document does **not** cover:
- What permissions are available (see L2-DES-SAFETY-001)
- The internal sandbox implementation
- Security Mode-specific approval rules

## Design Decisions

### DD-1: The agent requests approval through its tool calls; the system enforces

The agent expresses intent to perform a restricted action. The system evaluates the request against the current permission profile and approval policy, then either allows, denies, or prompts the user. The agent cannot bypass this flow.

**Decision**: All sensitive tool calls (shell command, file write, network access) route through a centralized `authorize_tool_request` path. No tool handler evaluates permissions independently.

### DD-2: Layered approval: hooks → cache → auto-review → user

Approval decisions should short-circuit as early as possible to minimize latency and decision fatigue. Each layer can approve or deny; only unresolved requests reach the next layer.

**Decision**: The approval flow is a four-layer pipeline. Any layer may produce an `Allow` or `Deny` decision. If a layer returns `Ask`, the request proceeds to the next layer.

### DD-3: Approval scopes reduce repeated prompts

When a user approves an action, they should be able to scope that approval so similar actions in the future don't re-prompt. The scope types map to what the cache can efficiently match.

**Decision**: Six scope types: `Once`, `Turn`, `Session`, `PathPrefix`, `Host`, `CommandPrefix`. `Tool` scope is also supported for MCP tool-level grants.

### DD-4: Escalation has two tiers

Not all blocked actions are equal. Sometimes the agent needs one additional path; sometimes it needs to run outside sandbox entirely. A two-tier model expresses this cleanly.

**Decision**:
- **Tier 1 — Additional Permissions**: The agent stays sandboxed but requests specific additional filesystem paths or network access for one command.
- **Tier 2 — Full Escalation**: The agent requests to run outside the sandbox entirely. Requires explicit justification and may offer a prefix rule for future auto-approval.

### DD-5: Auto-review is optional and configurable

An LLM-based auto-reviewer can approve low-risk actions without user intervention. It must fail closed (uncertain → prompt user). It is bounded by per-turn circuit breakers.

**Decision**: Auto-review is enabled when `approvals_reviewer = "auto_review"`. The reviewer returns `Approve`, `Deny`, or `Uncertain`. Deny is final; uncertain, timeout, provider failure, and invalid output fall through to the user prompt. Both escalation tiers are eligible for AutoReview.

## Architecture

### Approval Policy Levels

The `approval_policy` configuration key determines the default approval posture:

| Policy | Behavior |
|--------|----------|
| `never` | All commands are auto-approved. No approval prompts. Failures are returned to the agent without user escalation. |
| `on-request` (default) | The agent decides when to request approval. It uses per-call parameters (`sandbox_permissions`, `justification`) to signal the need. |
| `unless-trusted` | Only commands matching a pre-approved "safe commands" allowlist are auto-approved. Everything else requires user approval. |
| `on-failure` | Commands run sandboxed first. If they fail (sandbox error, network error), they are re-presented to the user for approval to run unsandboxed. (Deprecated in favor of `on-request`.) |
| `granular` | Each approval category is controlled individually with boolean toggles. Categories set to `false` are auto-rejected. |

#### Granular Approval Categories

When `approval_policy = "granular"`, the following categories may be individually toggled:

| Category | Default | Meaning |
|----------|---------|---------|
| `sandbox_approval` | `true` | Allow shell command approval requests (both additional-permissions and full escalation) |
| `rules` | `true` | Allow prompts triggered by exec-policy `prompt` rules |
| `skill_approval` | `false` | Allow prompts triggered by skill script execution |
| `request_permissions` | `false` | Allow prompts triggered by the `request_permissions` tool |
| `mcp_elicitations` | `true` | Allow MCP server elicitation prompts |

Categories set to `false` cause the corresponding requests to be automatically denied rather than shown to the user.

### Approval Decision Flow

```
Tool Call Request
       │
       ▼
┌──────────────────┐
│  Permission Mode  │  AutoApprove? → Allow. Deny? → Deny.
│  Override Check   │
└──────┬───────────┘
       │ (no override)
       ▼
┌──────────────────┐
│  Permission Check │  Action within current profile? → Allow.
│  (Profile Eval)   │  Action outside profile? → continue.
└──────┬───────────┘
       │ (needs approval)
       ▼
┌──────────────────┐
│  Approval Hooks   │  Matched hooks run. Any Deny? → Deny.
│  (if configured)  │  All Allow? → Allow.
│                   │  No hooks / no decision → continue.
└──────┬───────────┘
       │ (unresolved)
       ▼
┌──────────────────┐
│   Cache Lookup    │  Same tool+scope cached? → Allow.
│                   │  Denied and cached? → Deny.
└──────┬───────────┘
       │ (not cached)
       ▼
┌──────────────────┐
│   Auto-Reviewer   │  Reviewer = AutoReview? Run LLM check.
│  (if configured)  │  Approve? → Allow. Deny? → Deny.
│                   │  Uncertain? → continue.
└──────┬───────────┘
       │ (no auto-review decision)
       ▼
┌──────────────────┐
│   User Prompt     │  Show approval request in TUI.
│   (TUI Modal)     │  User response → Allow/Deny with scope.
└──────────────────┘
```

#### Stage Details

**1. Permission Mode Override**: The `PermissionMode` for the session may be set to `AutoApprove` or `Deny` directly. This is the outermost check and bypasses all other stages. It is settable via the `/permissions` command or session initialization.

**2. Permission Profile Check**: The active `PermissionProfile` is evaluated against the requested action. If the action (filesystem path, network host, command) falls within the allowed boundaries, it is allowed immediately without any approval prompt. This includes checking:
- Filesystem read/write access against the sandbox entries
- Network access against the network policy
- Command execution against exec-policy allow rules

**3. Approval Hooks**: User-configured hook scripts can intercept approval requests before they reach the user. Hooks receive the tool name, input, and permission mode. They may return `allow`, `deny`, or no decision. Any deny from any hook is final (fail-closed). The most-specific allow wins; if no hook decides, the request continues.

**4. Cache Lookup**: Previous approval decisions with scope broader than `Once` are cached. The cache is checked against:
- `Turn` scope: same turn ID, same tool and resource
- `Session` scope: same session ID, same tool and resource
- `PathPrefix` scope: requested path starts with the cached prefix
- `Host` scope: requested host matches the cached host
- `CommandPrefix` scope: requested command starts with cached prefix tokens

**5. Auto-Reviewer**: When `approvals_reviewer = "auto_review"`, an LLM call evaluates the approval request. The reviewer receives a compact prompt with the action details, permission context, safety rules, and the requested sandbox tier plus exact overlay paths/network access. It must return a structured JSON decision. Requests using either `with_additional_permissions` or `require_escalated` enter this stage; hard policy and hook denials remain earlier and final.

The reviewer is bounded by circuit breakers:
- Maximum 3 consecutive denials per turn before the turn is interrupted.
- Maximum 10 denials in the last 50 auto-review requests before the turn is interrupted.
- On circuit-breaker trip, the agent receives a message explaining that too many actions were rejected and must send a final message to the user.

**6. User Prompt**: The final stage. A `PendingApproval` is created and an approval request is sent to the TUI. The user sees:
- What action is proposed (tool name, command, path, host)
- Why it needs approval (what permission boundary is exceeded)
- What options are available (approve with various scopes, deny, cancel)

The user's decision is routed back to the runtime, which applies the chosen scope to the cache and resumes or rejects the tool call.

### The `authorize_tool_request` API

This is the central entry point for the approval mechanism. It receives a `ToolPermissionRequest` and returns a `PermissionDecision`.

```rust
fn authorize_tool_request(
    profile: &RuntimePermissionProfile,
    policy: &ApprovalPolicy,
    cache: &ApprovalCache,
    request: &ToolPermissionRequest,
) -> PermissionDecision;
```

`PermissionDecision` variants:
- `Allow` — Action is permitted, proceed.
- `Deny { reason }` — Action is denied, agent receives the reason.
- `Ask { approval_id, message, available_scopes }` — User must decide.

### Escalation Model

#### Tier 1: Additional Permissions

The agent calls a tool with `sandbox_permissions: "with_additional_permissions"` and provides an `additional_permissions` block containing the extra filesystem paths or network access needed. The command runs inside the sandbox with the merged permissions. This is preferred over full escalation because the sandbox still constrains the command.

#### Tier 2: Full Escalation

The agent calls a tool with `sandbox_permissions: "require_escalated"` and a `justification` string. The command runs outside the sandbox entirely. Full escalation:
- Enters the configured reviewer in `auto_review` mode; an approval produces a one-call unsandboxed grant. Uncertain, reviewer failure, or invalid reviewer output goes to the user.
- May include a `prefix_rule` suggestion for future auto-approval.
- The `prefix_rule` becomes an exec-policy allow rule if the user approves.

#### Prefix Rules

A prefix rule is a command prefix (e.g. `["npm", "run", "dev"]`) that, once approved, allows matching commands to run without further approval. Prefix rules:
- Are matched against the start of the command tokens
- Are evaluated per-segment (control operators like `|`, `&&`, `||`, `;` split the command)
- Use exact token matching
- Cannot be requested for destructive commands (`rm`, etc.) or commands using heredocs

### Approval Scope Types

| Scope | Lifetime | Match Key |
|-------|----------|-----------|
| `Once` | This single invocation | N/A (not cached) |
| `Turn` | Rest of the current turn | `(turn_id, tool_name, resource)` |
| `Session` | Rest of the session | `(session_id, tool_name, resource)` |
| `PathPrefix` | Matches actions on paths with this prefix | `(path, session_id or turn_id)` |
| `Host` | Matches network actions to this host | `(host, session_id or turn_id)` |
| `CommandPrefix` | Matches commands starting with these tokens | `(command_prefix, session_id or turn_id)` |
| `Tool` | Matches this MCP tool from this server | `(server_name, tool_name, session_id)` |

### Approval Cache

The cache stores previous approval decisions to avoid re-prompting. It is organized as a set of matchable keys.

**Turn-level cache**: Cleared at the end of each turn. Stores approvals scoped to the current turn.

**Session-level cache**: Persists for the session duration. Stores approvals scoped to the session.

**Cache structure**:
```
ApprovalCache {
    turn_tools: HashSet<(String, ResourceKind)>,
    session_tools: HashSet<(String, ResourceKind)>,
    turn_hosts: HashSet<String>,
    session_hosts: HashSet<String>,
    turn_path_prefixes: HashSet<PathBuf>,
    session_path_prefixes: HashSet<PathBuf>,
    turn_command_prefixes: HashSet<Vec<String>>,
    session_command_prefixes: HashSet<Vec<String>>,
}
```

Cache entries are created when the user selects a scope broader than `Once`. Deny decisions are also cached: a denied action with session scope is auto-denied for the rest of the session.

### Auto-Reviewer

The auto-reviewer is an LLM-based component that evaluates approval requests when `approvals_reviewer = "auto_review"`.

**Input**: A structured prompt containing:
- The tool name and action summary
- The resource type (file read, file write, shell exec, network)
- The specific path, host, command, or target
- The agent's justification
- The current permission profile summary

**Output**: A JSON decision:
```json
{"decision": "approve", "rationale": "scoped npm install within workspace"}
{"decision": "deny", "rationale": "attempts to write to /etc"}
{"decision": "uncertain", "rationale": "needs more context about user intent"}
```

**Safety properties**:
- Fail-closed: invalid output goes directly to the user; transient provider failures and timeouts are retried once, then escalate to the user.
- Tier 1 approvals preserve the base sandbox and are cached only with the exact normalized overlay. Tier 1 grants cannot be reused for Tier 2, and a Tier 1 sandbox denial never silently retries with sandbox disabled.
- Circuit breaker: excessive denials cause the turn to be interrupted.
- The reviewer prompt includes explicit safety rules (deny destructive commands, credential access, privilege escalation, ambiguous high-impact actions).

### TUI Approval Overlay Contract

The TUI presents approval requests as a modal overlay. The overlay receives:

| Field | Description |
|-------|-------------|
| `approval_id` | Unique identifier for this request |
| `action_summary` | One-line human-readable description |
| `justification` | Agent's reason for the request (optional) |
| `resource` | Resource kind (FileRead, FileWrite, ShellExec, Network) |
| `available_scopes` | Scope options the user can choose |
| `path` | Filesystem path (for file operations) |
| `host` | Network host (for network operations) |
| `target` | Command or action target |

The user can choose:
- **Approve Once** — Allow this single action
- **Approve for Turn** — Allow similar actions for the rest of this turn
- **Approve for Session** — Allow similar actions for the session
- **Approve for Path/Host/Command** — Allow actions matching this prefix
- **Deny** — Reject this action (and optionally remember for session)
- **Cancel** — Abort the agent's current request

**Keyboard shortcuts** (default keymap):
- `y` — Approve Once
- `s` — Approve for Session
- `n` — Deny
- `Esc` — Cancel (MCP elicitation only) or Deny
- `Ctrl+C` — Cancel current request and clear queue

The overlay supports a queue of pending approval requests. When multiple requests accumulate (e.g. from parallel tool calls), they are presented one at a time in FIFO order.

### Per-Tool Approval Overrides

Individual tools (especially MCP server tools) can have their own approval mode configured independently of the global policy:

| Per-Tool Mode | Behavior |
|---------------|----------|
| `auto` | Tool calls are always auto-approved, even under restrictive global policies |
| `prompt` | Tool calls always require user approval, even under permissive global policies |
| `approve` | Default — follow the global approval policy |

This is configured in the tool/server config section:

```toml
[servers.my-server.tools]
"destructive_tool" = { approval_mode = "prompt" }
```

### Approval Lifecycle Events

The approval mechanism emits structured events at key points:

| Event | When |
|-------|------|
| `approval_request_created` | A new approval request is queued |
| `approval_hook_completed` | Each hook finishes execution |
| `approval_auto_reviewed` | Auto-reviewer produces a decision |
| `approval_user_prompted` | Request is shown to user |
| `approval_resolved` | Final decision is made (allow/deny/cancel) |
| `approval_cached` | Decision is stored in the approval cache |

These events feed the transcript/history system, observability, and TUI state updates.

### Agent Instructions

The approval policy influences what instructions the agent receives about how to request approval. The agent receives a contextual "permissions instructions" block injected into the system prompt that describes:
- The active permission profile (what it can do)
- The approval policy (when it must ask)
- Whether the `request_permissions` tool is available
- Whether the auto-reviewer is active and how it behaves
- Currently approved command prefixes

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-APP-003 | 1 | specs/L1/L1-REQ-APP-003-safety.md | Defines approval flow, escalation, user consent, and scoped grants. |
| refines | L1-REQ-SEC-001 | 1 | specs/L1/L1-REQ-SEC-001-security-mode.md | Security Mode preserves normal approval behavior. |
| related-to | L2-DES-SAFETY-001 | 1 | specs/L2/safety/L2-DES-SAFETY-001-permission-system.md | Approval references permission profiles for boundary checks. |
| related-to | L2-DES-TUI-CMD-007 | 1 | specs/L2/tui/slash-commands/L2-DES-TUI-CMD-007-permissions.md | /permissions command changes the active policy mode. |
| related-to | L2-DES-TUI-004 | 1 | specs/L2/tui/L2-DES-TUI-004-streaming-transcript-and-state.md | Approval states are visible in the transcript. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | Approval events travel over the client-server protocol. |
| related-to | L2-DES-APP-005 | 2 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Approval policy is part of the config schema. |
| specified-by | L3-BEH-CORE-004 | 2 | specs/L3/core/L3-BEH-CORE-004-permission-approval.md | L3 defines approval decision pipeline, approval cache, auto-reviewer, escalation tiers, and prefix rules. |
| related-to | L3-BEH-SAFETY-002 | 2 | specs/L3/safety/L3-BEH-SAFETY-002-approval-pipeline.md | L3 defines approved grant application at sandbox and process-boundary enforcement. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial approval mechanism design. |
