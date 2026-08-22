---
artifact_id: L2-DES-APP-008-MAP
revision: 2
status: Approved
active_baseline: yes
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-22
---

# L2-DES-APP-008-MAP — Legacy → Canonical Method Mapping (Phase A Appendix)

Appendix to L2-DES-APP-008. Exhaustive inventory of the remaining legacy RPC
surface (the former flat-RPC dispatch surface) and its disposition
under protocol unification. Methods removed during Phase E are retained in the
tables as historical deletion records.

## Disposition Legend

- **EXISTS** — canonical counterpart exists (possibly renamed); Phase B verifies param shapes and converts the legacy handler into a translator (DD-4).
- **MERGE-INTO** — folded into canonical `session/metadata/update` (`SessionSettings` patch); no standalone canonical method.
- **DUAL-SHAPE** — same method string exists in both surfaces with divergent param shapes; converge on the canonical shape.
- **RESTRUCTURE** — the canonical model handles the concern differently (e.g. server-request/client-response instead of a respond method).
- **GAP** — no canonical counterpart; requires a new canonical definition (designed in Phase B per domain).
- **REDESIGN** — canonical deliberately models this differently; migration requires a product-level decision, not a translation.
- **REMOVED** — the legacy façade, schema binding, dispatch route, and first-party wrapper are deleted; the canonical counterpart remains where applicable.

## Session Domain

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `session/metadata/update` | `session/metadata/update` (`SessionSettingsPatch` patch, `expected_version`) | ✅ legacy flat shape, façade, and dual-shape dispatch removed 2026-08-11; canonical persist-first write path is the only request shape; TUI fully migrated |
| `session/permissions/update` | `SessionSettingsPatch.permission_profile` via `session/metadata/update` | ✅ legacy route, façade, dispatch variant, and schema binding removed 2026-08-10; TUI migrated |
| `session/sandbox_profile/update` | `SessionSettingsPatch.sandbox_profile` via `session/metadata/update` | ✅ legacy route, façade, dispatch variant, and schema binding removed 2026-08-10; previously-unpersisted sandbox changes now persist via field-level settings lines |
| `session/title/update` | `title` PatchField via `session/metadata/update` | ✅ legacy route, façade, dispatch variant, and schema binding removed 2026-08-11; canonical title patch persists `SessionTitleUpdated` lines and rejects `Null` clearing; TUI and desktop migrated |
| `session/compaction/update` | `SessionSettingsPatch.effective_context_window` via `session/metadata/update` | ✅ legacy route, façade, dispatch variant, and schema binding removed 2026-08-11; canonical path persists globally, fans out to loaded sessions, and echoes the clamped value |
| `session/start` / ACP `session/new` | `session/new` | ✅ served 2026-08-09 (idempotency-key replay, rollout-backed canonical snapshot); TUI migrated. Routing note: the ACP adapter shares this method name; dispatch keys on the negotiated connection protocol surface (L2-DES-APP-009 DD-6) |
| ACP `session/list` | `session/list` | ✅ served 2026-08-09 (offset-paged canonical snapshots; sessions with unreadable rollouts skipped); TUI picker migrated |
| — (no legacy counterpart) | `session/read` | ✅ served 2026-08-09 (rollout-backed canonical snapshot for one session; completes the canonical session CRUD surface: new/read/resume/list/delete) |
| ACP `session/delete` | `session/delete` | ✅ served 2026-08-09 (same delete-tree side effects and SessionDeleted broadcast as the ACP path); TUI migrated |
| `session/resume` | `session/resume` (hydrate) + `session/items/list` (transcript) + `session/queue/list` (pending input) | ✅ served 2026-08-02; TUI migrated 2026-08-02 — all four call sites (initial resume, session switch, fork-resume, input-history browse) now restore via the canonical trio with a shared `restore_session_canonical` helper. Transitional approximations documented in code: `prompt_token_estimate` falls back to total input tokens; last-query meter starts at zero (no canonical source yet); plan/file-change history metadata partially degrades |
| `session/fork` | `session/fork` | ✅ served 2026-08-02 (dual-shape via camelCase `sessionId`; canonical `atTurnId` maps to the legacy user-turn index server-side with the fork machinery's own user-turn counting rule); TUI migrated (resolves index → turn id via `session/turns/list`, then canonical fork) |
| `session/rollback` | `session/rollback/preview` + `session/rollback/commit` | ✅ legacy route, client façade, dispatch variant, and schema binding removed 2026-08-10; TUI uses preview → commit → canonical transcript restore. With a git checkpoint the commit also restores the workspace; without one it stays history-only. |
| `session/compact` | `session/compact/start` | ✅ legacy route, façade, and dispatch removed; canonical compaction start is served and used by the TUI |

## Turn Domain

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `turn/start` | `turn/start` | DUAL-SHAPE — ✅ served 2026-08-02 (idempotency key, `RejectActive` busy semantics, in-process idempotent replay, input conversion with skill inputs rejected pending the skill domain); TUI migrated (settings push before admission; skill inputs fall back to legacy) |
| `turn/interrupt` | `session/interrupt` | ✅ removed 2026-08-12; Native and Desktop use the scoped session interrupt operation, while ACP v1 retains standard `session/cancel` |
| `turn/shell_command` | `task/start` (`kind: "process"`, attached to the active turn) | ✅ legacy route, façade, and dedicated shell-turn path removed 2026-08-10; Shell Mode uses `command/exec` / `task/*` |
| `session/compact` | `session/compact/start` | ✅ served 2026-08-02 (canonical turn snapshot result; busy rejects); TUI migrated |

## Queue / Steer Domain

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `session/queue/push` (Ctrl+S enqueue) | `session/queue/push` | ✅ served; TUI migrated (canonical `rpc_turn` types end-to-end) |
| `session/queue/list` | `session/queue/list` | ✅ served; TUI migrated (also part of the resume transcript trio) |
| `session/queue/update` | `session/queue/update` | ✅ served; TUI migrated |
| `session/queue/remove` (Ctrl+D dequeue) | `session/queue/remove` | ✅ served; TUI migrated |
| `turn/steer` | `session/queue/steer` | ✅ served; TUI migrated — steer rides the queue by design: queued input is not yet an item of any turn (canonical `UserMessage.entry` model absorbs steer). Direct `turn/steer` and `turn/read` remain registered in the canonical method set but unserved (no consumer); revisit if a client needs them |

## Events / Subscriptions

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `events/subscribe` | `subscription/create` + `subscription/update` + `subscription/ack` + `subscription/unsubscribe` | ✅ legacy route, façade types, dispatch variant, and schema binding removed 2026-08-10; observer coverage migrated to canonical selectors. The internal session filter remains only for the sessionless command-output compatibility path |

## Goal Domain

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `goal/create` / `goal/set` (create modes) | `session/goal/set` | ✅ legacy client façade, dispatch variants, and schema bindings removed 2026-08-10; TUI and desktop use canonical goal set/read/update flows |
| `goal/pause` / `goal/resume` / `goal/complete` / `goal/cancel` / `goal/clear` | `session/goal/pause` / `resume` / `complete` / `cancel` / `clear` | ✅ legacy client façade, dispatch variants, and schema bindings removed 2026-08-10; canonical transitions retain internal domain adapters |
| `goal/status` | `session/goal/read` | ✅ legacy client façade, dispatch variant, and schema binding removed 2026-08-10; TUI and desktop read canonical goal projections |

## Exec / Task Domain

Per L2-DES-APP-008 DD-7 (human-approved 2026-08-02), exec and agents are unified as **tasks**: session-owned background work units addressed by `item_id` with `kind: "process" | "agent"`. Existing canonical `task/read`, `task/write_stdin`, `task/interrupt` are retained; `task/start`, `task/list`, `task/message`, `task/resize` are added.

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `command/exec` | `task/start` (`kind: "process"`) | RESOLVED (was REDESIGN; new canonical verb) |
| `command/exec/write` | `task/write_stdin` | EXISTS (renamed) |
| `command/exec/resize` | `task/resize` | RESOLVED (was GAP; new canonical verb) |
| `command/exec/terminate` | `task/interrupt` | EXISTS (renamed) |
| — (live output polling was event-only) | `task/read` + `task/list` | ✅ served 2026-08-09 (DD-7 unified read/list: process tasks project as `BackgroundTask` items with a bounded retained output tail (16 KiB) and terminal snapshots after exit; agent tasks reuse the SubAgent projection; no TUI consumer yet) |

## Agent Domain

Under DD-7, a sub-agent is a task with `kind: "agent"` backed by a child session (the task item holds the child `session_id`). This deliberately supersedes canonical's earlier no-public-spawn stance (`crates/protocol/src/native/rpc_turn.rs:229`): L1-REQ-AGENT-004 requires user-requested subagent creation, so a public start path exists as `task/start`.

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `agent/spawn` | `task/start` (`kind: "agent"`) | RESOLVED (was REDESIGN) |
| `agent/send_message` | `agent/message` | RESOLVED (renamed; internally a child-session turn) |
| `agent/wait` | `agent/read` / `task/read` + `subscription/*` | RESOLVED (was REDESIGN) |
| `agent/list` | `agent/list` / `task/list` | RESOLVED (renamed) |
| `agent/status` | `agent/read` / `task/read` | RESOLVED (renamed) |
| `agent/close` | `agent/cancel` / `task/interrupt` | RESOLVED (renamed) |

## Model / Provider Domain

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `model/catalog` | `model/list` | ✅ canonical `model/list` served; legacy façade, schema binding, and handler removed 2026-08-10 |
| `model/config` | `model/preferences/read` | ✅ canonical preferences served and desktop migrated; legacy façade, schema binding, and handler removed 2026-08-10 |
| `model/config/set` | `model/preferences/write` | ✅ canonical patch write served and desktop migrated; legacy façade, schema binding, and handler removed 2026-08-10 |
| `model/saved` | `model/preferences/read` | ✅ legacy façade, schema binding, and handler removed; canonical preferences supersede the unused saved-model listing |
| `provider/list` | `provider/list` (proposal, Open Decision #11) | GAP → **proposal drafted 2026-08-09** |
| `provider/validate` | `provider/validate` (proposal, Open Decision #11) | GAP → **proposal drafted 2026-08-09** |
| `provider/upsert` | `provider/upsert` (proposal, Open Decision #11) | GAP → **proposal drafted 2026-08-09** |

## Skills / MCP / Context

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `skills/list` | `skill/list` | ✅ canonical served + TUI migrated; legacy façade, schema binding, and handler removed 2026-08-10 |
| `skills/set_enabled` | `skill/set_enabled` | ✅ canonical served + TUI migrated; legacy façade, schema binding, and handler removed 2026-08-10 |
| `skills/changed` | `skill/list` with `forceReload` | ✅ legacy notification route removed 2026-08-10; callers explicitly reload the canonical catalog |
| `mcp/list` / `mcp/tools` / `mcp/set_enabled` | same names in canonical | ✅ verified 2026-08-09 (server handlers and the TUI both use the canonical `rpc_admin` types on the same wire names) |
| `context/usage/read` | same name in canonical | ✅ verified 2026-08-09 (server uses canonical `rpc_admin` types; no TUI consumer — the TUI is event-driven via `ContextUsageUpdated`) |

## Input / Approval

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `request_user_input/respond` | `userInput/request` (server request) + JSON-RPC response | ✅ removed 2026-08-11; canonical clients answer the reverse request through the pending JSON-RPC registry, and no first-party client emits the old response method |
| `approval/respond` (notification) | `approval/command/request` + `approval/fileChange/request` + `approval/permission/request` + client response | ✅ served; DD-8 mixed-surface fan-out sends Native controllers an `Item::Approval` reverse request before publishing its waiting item, while ACP controllers retain `session/request_permission`. The Native client registers the request before dispatching the typed item to the TUI, and answers with `ApprovalRespondParams`. |

## Search / Workspace / Edit

| Legacy method | Canonical counterpart | Disposition |
|---|---|---|
| `search/start` / `search/update` / `search/cancel` | `search/start` / `search/update` / `search/cancel` (same names, camelCase canonical types) | ✅ legacy client façade, dispatch variants, and schema bindings removed 2026-08-10; TUI and desktop use canonical requests. Connection-local notifications remain a separate event-shape cleanup |
| `workspace/changes/read` | `workspace/changes/read` (same name, camelCase canonical types) | ✅ served 2026-08-09 (desktop client is the confirmed consumer; dual-shape via camelCase `sessionId`, surface-agnostic; legacy enums reused with snake_case values inside camelCase payloads — accepted inconsistency); desktop TS migrated 2026-08-09 (camelCase on the wire, views converted back to the generated shape for the renderer) |
| `message/editPrevious` | `session/message/edit` | ✅ canonical edit served 2026-08-09; legacy façade, schema binding, and dispatch route removed 2026-08-10. Canonical `itemId` + `expectedRevision` and `SessionMessageEditResult` are now the only external edit surface. |

## Summary

| Disposition | Count | Methods |
|---|---:|---|
| EXISTS (rename/verify) | 20 | session/resume, session/fork, session/rollback(→2), goal/*(8), skills/list, skills/set_enabled, model/catalog, mcp/*(3), context/usage/read, command/exec/write, command/exec/terminate |
| MERGE-INTO `session/metadata/update` | 5 | session/metadata/update(settings part), session/permissions/update, session/sandbox_profile/update, session/title/update, session/compaction/update |
| DUAL-SHAPE | 2 | session/metadata/update, turn/start |
| RESTRUCTURE | 2 | events/subscribe, approval/respond |
| RESOLVED via unified task model (DD-7) | 10 | agent/spawn, agent/send_message, agent/wait, agent/list, agent/status, agent/close, command/exec, command/exec/resize, turn/shell_command (+ `task/*` verb family) |
| GAP (new canonical needed) | 11 → 1 open | ✅ search/*(3), message/editPrevious, model/preferences, and provider/* canonical surfaces served; remaining open GAP: workspace/changes/read (no in-repo consumer — TS desktop/web path needs confirmation) |

(Counts overlap where a method both merges and dual-shapes; the 52-variant legacy surface is fully covered by the rows above.)

## Phase E Consumer Inventory (audit 2026-08-10)

This is a repository-local production-consumer audit. It distinguishes an actual
caller from a method wrapper that merely remains exposed by `devo-client`; the
latter is still a Phase E API-removal obligation, but it is not evidence that a
first-party application currently sends that method. The audit covered
`crates/tui`, `crates/cli`, `apps/desktop` (excluding generated bindings and
tests), and `apps/web`, plus the public `StdioServerClient` /
`WebSocketServerClient` façades in `crates/client`.

Legend:

- **TUI-L / Desktop-L** — production code still sends a legacy-shaped wrapper
  or transitional route (a `fallback` annotation means it is only a
  compatibility path).
- **TUI-C / Desktop-C** — production code uses the canonical façade/shape for
  the same concern; the legacy method is not a deletion blocker for that
  caller.
- **Client-API** — the public `devo-client` transport façade still exposes a
  method that emits the legacy path. This is an API-surface dependency, not an
  in-repo call site by itself.
- **—** — no production caller found in the audited trees.

| Legacy method | Production consumers found | Phase E note |
|---|---|---|
| `session/metadata/update` | TUI-C, Desktop-C, Client-API | ✅ legacy flat params, façade, and dual-shape dispatch removed 2026-08-11; callers use canonical params. |
| `session/permissions/update` | TUI-C, Client-API | ✅ legacy route, façade, dispatch variant, and schema binding removed 2026-08-10; TUI uses `session/metadata/update` settings patch. |
| `session/sandbox_profile/update` | Client-API | ✅ legacy route, façade, dispatch variant, and schema binding removed 2026-08-10; no first-party caller remained. |
| `session/title/update` | TUI-C, Desktop-C, Client-API | ✅ legacy route and façade removed 2026-08-11; TUI and desktop use the canonical metadata patch. |
| `session/resume` | TUI-C | `devo-client`'s `session_resume` is the ACP/canonical session resume path. |
| `session/fork` | TUI-C, Client-API | TUI uses canonical fork. |
| `session/rollback` | TUI-C, Client-API | ✅ legacy client route and dispatch removed 2026-08-10; TUI uses canonical preview/commit. |
| `session/compact` | TUI-C | Legacy client façade and dispatch route removed 2026-08-10; TUI uses canonical `session/compact/start`. |
| `session/compaction/update` | Client-API | ✅ legacy route, façade, dispatch variant, and schema binding removed 2026-08-11; no first-party caller remained. |
| `skills/list` | TUI-C | Legacy façade, schema binding, and handler removed; TUI uses canonical `skill/list`. |
| `skills/changed` | — | Legacy notification route removed; canonical `skill/list` supports explicit `forceReload`. |
| `skills/set_enabled` | TUI-C | Legacy façade, schema binding, and handler removed; TUI uses canonical `skill/set_enabled`. |
| `model/catalog` | — | Legacy façade, schema binding, and server handler removed; canonical `model/list` remains served. |
| `model/config` | Desktop-C | Legacy façade, schema binding, and server handler removed; desktop uses canonical preferences. |
| `model/config/set` | Desktop-C | Legacy façade, schema binding, and server handler removed; desktop uses canonical preferences. |
| `model/saved` | — | Legacy façade, schema binding, and server handler removed; canonical model preferences supersede it. |
| `command/exec` | TUI-L, Client-API | TUI's sessionless shell-command path still uses the transitional direct route; canonical sessionless task semantics remain the deletion prerequisite. |
| `command/exec/write` | — | Legacy client façade and dispatch variant removed; canonical `task/write_stdin` remains served. |
| `command/exec/resize` | — | Legacy client façade and dispatch variant removed; canonical `task/resize` remains served. |
| `command/exec/terminate` | — | Legacy client façade and dispatch variant removed; canonical `task/interrupt` remains served. |
| `message/editPrevious` | — | Legacy façade, schema binding, and dispatch route removed 2026-08-10; canonical `session/message/edit` remains served. |
| `turn/start` | TUI-C, Desktop-C, Client-API | TUI and desktop send canonical params; the public Rust client still exposes a legacy-shaped compatibility wrapper. |
| `turn/shell_command` | — | Legacy client façade, schema binding, dispatch route, and dedicated shell-turn execution path removed 2026-08-10; Shell Mode uses `command/exec` and canonical `task/*`. |
| `turn/interrupt` | — | Removed 2026-08-12; Native clients use `session/interrupt` with `session`, `task`, or `command` scope. |
| `session/interrupt` | TUI-C, Desktop-C, Client-API | Scoped Native interrupt application command; Desktop reaches the same path through an explicit ACP adapter extension. |
| `workspace/changes/read` | Desktop-C | Desktop uses the canonical camelCase shape on the same method string. |
| `request_user_input/respond` | Client-API | Removed 2026-08-11; first-party clients answer canonical `userInput/request` reverse requests directly. |
| `search/start` | TUI-C, Desktop-C | Legacy façade and dispatch removed; both first-party clients use canonical search. |
| `search/update` | TUI-C, Desktop-C | Same canonical surface as `search/start`. |
| `search/cancel` | TUI-C, Desktop-C | Same canonical surface as `search/start`. |
| `events/subscribe` | — | Legacy route and schema removed; first-party callers use canonical subscriptions. |
| `goal/create` | — | Legacy façade, dispatch variant, and schema binding removed; TUI uses canonical `session/goal/set`. |
| `goal/set` | — | Legacy façade, dispatch variant, and schema binding removed; in-place edits use canonical `session/goal/update`. |
| `goal/pause` | — | Legacy façade, dispatch variant, and schema binding removed; clients use canonical goal transitions. |
| `goal/resume` | — | Legacy façade, dispatch variant, and schema binding removed; clients use canonical goal transitions. |
| `goal/complete` | — | Legacy façade, dispatch variant, and schema binding removed; clients use canonical goal transitions. |
| `goal/cancel` | — | Legacy client route and schema registration removed; canonical `session/goal/cancel` is served. |
| `goal/clear` | — | Legacy façade, dispatch variant, and schema binding removed; desktop and TUI use canonical goal transitions. |
| `goal/status` | — | Legacy façade, dispatch variant, and schema binding removed; desktop and TUI read canonical goal state. |
| `agent/spawn` | TUI-C, Client-API | ✅ legacy route, client façade, dispatch variant, and schema binding removed 2026-08-10; canonical `task/start(kind: "agent")` remains. |
| `agent/send_message` | TUI-C | ✅ legacy route, client façade, dispatch variant, and schema binding removed 2026-08-10; canonical `agent/message` remains. |
| `agent/wait` | TUI-C | ✅ legacy route, client façade, dispatch variant, and schema binding removed 2026-08-10; built-in tools retain the internal coordinator and canonical reads use `agent/read`/`task/read`. |
| `agent/list` | TUI-C, Client-API | ✅ legacy route, client façade, dispatch variant, and schema binding removed 2026-08-10; canonical `agent/list` remains. |
| `agent/status` | — | ✅ legacy route and schema registration removed; canonical `agent/read`/`task/read` retain the shared internal status projection. |
| `agent/close` | TUI-C, Client-API | ✅ legacy route, client façade, dispatch variant, and schema binding removed 2026-08-10; canonical `agent/cancel` remains. |
| `provider/list` | TUI-C, Desktop-C, Client-API | TUI and desktop use the canonical camelCase provider result. The Rust client facade now sends the direct canonical method. |
| `provider/validate` | TUI-C, Desktop-C, Client-API | TUI and desktop use canonical camelCase params; the legacy client wrapper and server adapter are removed. |
| `provider/upsert` | TUI-C, Desktop-C, Client-API | TUI and desktop use canonical camelCase params; the legacy client wrapper and server adapter are removed. |
| `mcp/list` | TUI-C, Client-API | The TUI and Rust client use the canonical method directly; no extension alias is registered. |
| `mcp/tools` | TUI-C, Client-API | The TUI and Rust client use the canonical method directly; no extension alias is registered. |
| `mcp/set_enabled` | TUI-C, Client-API | The TUI and Rust client use the canonical method directly; no extension alias is registered. |
| `context/usage/read` | — | Canonical handler retained, but no legacy registration remains; TUI is event-driven and has no request caller. |

There are no CLI or `apps/web` production consumers of the legacy methods in
this audit. Rust protocol registration and `acp/ts.rs` schema entries are
server/adapter definitions, not consumers. The client and server websocket
integration tests now use direct canonical method names. The provider/MCP
client façade methods were first-party only and are now removed; remaining public legacy methods still need an
explicit external-consumer compatibility decision.

Recommended deletion batches from this inventory:

1. **Canonical-only dead surface:** methods marked `—`, after deleting stale
   adapter schemas and contract fixtures together.
2. **TUI compatibility tails:** `command/exec` and the skill-input/user-input
  fallbacks. Provider and MCP are complete.
3. **Desktop compatibility tails:** the legacy user-input response and
   sessionless command start; goal controls and reference search are now
   canonical.
4. **Client façade removal:** continue deleting unused legacy methods from
   both transports as each repository consumer migrates; provider/MCP,
   model, command control, goal, and search batches are complete.

## Recommended Phase B Order

1. **Session domain** (highest TUI traffic; settings MERGE-INTO rides here as the pilot per L2-DES-CONV-002).
2. **Turn + events domains** (turn/start DUAL-SHAPE, `events/subscribe` → `subscription/*`; unblocks the TUI event cutover in Phase C).
3. **Goal + exec/task domains** (renames; `task/*` verb family built here per DD-7).
4. **Agent domain** (rides the DD-7 task model built for exec; no separate product decision needed anymore).
5. **Model/provider/search/workspace/edit domains** (GAP-heavy; canonical definitions designed here).

## Canonical Registry Coverage (audit 2026-08-09)

Every `NATIVE_METHODS` entry is either served or explicitly parked:

- **Served**: all session CRUD (`new`/`read`/`resume`/`list`/`delete`), `runtime/ping` (served 2026-08-09), metadata/settings, turn (`start`/`interrupt`), queue (5 verbs), goal (8), task (6), agent (5), fork, rollback (2), items/turns list, subscription (4), compact, search (3), mcp (3), model/list + model/preferences, context/usage, and canonical skill methods.
- **Parked, by design**: `turn/steer` and `turn/read` (no consumer — steer rides `session/queue/steer`), `session/archive` (feature unimplemented on both surfaces), `session/cwd/change` (no legacy counterpart, no consumer).
- **Parked, needs a design decision**: `credential/list|set|delete` and `permission/profile/read|update` (new admin surface with secrets/permission implications — not renames; requires a dedicated design before serving), `tool/list` (no legacy counterpart), `model/list` (#7), `skill/list`/`skill/set_enabled` (#4).

## Open Decisions (need human call)

1. ~~`agent/spawn` / `agent/wait`~~ — **RESOLVED 2026-08-02**: unified task model (L2-DES-APP-008 DD-7); public agent start exists as `task/start(kind: "agent")`.
2. ~~`turn/shell_command` / `command/exec` start~~ — **RESOLVED 2026-08-02**: `task/start(kind: "process")` (DD-7).
3. ~~**NEW 2026-08-02 — in-place goal edit**~~ — **RESOLVED 2026-08-09** (option (b), ratified + implemented): canonical `session/goal/update` with `GoalPatch` (`objective`, `status` limited to active/paused/completed — system-computed statuses rejected, `tokenBudget` as PatchField with Null rejected) + `expectedGoalId` precondition + idempotency key. TUI `/goal` edit flow migrated (legacy `goal/set` UpdateExisting retired from the TUI).
4. ~~**NEW 2026-08-02 — skill domain vocabulary**~~ — **RESOLVED 2026-08-09** (ratified (a)+(c), implemented): canonical `skill/list` gained `cwd` (workspace scope) and `force_reload`; canonical `SkillInfo` carries full parity with `SkillRecord` (id/name/description/shortDescription/interface/path/enabled/source/scope/pluginId); `skill/set_enabled` is keyed by `path`. TUI migrated (picker + toggle flows convert canonical records back to `SkillRecord`). Note: legacy `skill/set_enabled` no-ops on unknown paths returning the current list — canonical preserves this. `skills/changed` stays legacy pending #5.
5. ~~`skills/changed`~~ — **RESOLVED 2026-08-10**: remove the unused client notification route; canonical `skill/list` accepts `forceReload` for explicit rediscovery.
6. Legacy extension-method prefix and session-less event routing: retired in Phase E; no first-party consumer remains.
7. ~~**NEW 2026-08-09 — `model/list` shape**~~ — **RESOLVED 2026-08-09** (option (a), ratified): canonical `ModelInfo` enriched to full parity with `ModelCatalogEntry` and `model/list` served from the same catalog source. The `model/config*`/`model/saved` preferences part moves to its own item (see #12).
8. ~~**NEW 2026-08-09 — event straggler vocabulary**~~ — **RESOLVED 2026-08-09** (ratified + implemented): retry projects with additive `provider`/`model`/`phase` on `model/queryRetrying` (+ `ModelQueryRetryPhase` enum; `max_attempts` required, missing → legacy path); usage meter projects to live typed `turn/usage/updated` (per-query `TurnUsage` + `sessionTotals` added so the live session meter does not regress); plan projects as full `Plan` item on `item/updated` (revision folding, no plan-delta); compaction de-straggled via emit-site enrichment. TUI typed consumption for all four is in place (`worker.rs` typed fast-path); `_meta` embedding and the typed-first flip are unblocked (#9).
9. **NEW 2026-08-09 — typed-first flip timing** (L2-DES-APP-009 DD-4): when to flip first-party connections to typed-first projection and delete `_meta.devo.original_event` embedding + `acp_events.rs` dispatch. Depends on decision 8 for the stragglers the TUI still renders (usage meter, plan, retry).
10. ~~**NEW 2026-08-09 — `session/message/edit` (message/editPrevious canonical design)**~~ — **RESOLVED 2026-08-09** (ratified + implemented as proposed): accepted edit = same `UserMessage` item revision + 1; `session/message/edit` served with `expectedRevision` optimistic concurrency; `turn/superseded` canonical durable event added and projected. Sub-question answers landed: (a) auto-regenerate kept (legacy behavior); (b) UI marker is a client concern; (c) `turn/superseded` is a durable fact. No in-repo consumer yet — TUI/desktop wiring is future feature work, not migration debt.
11. ~~**NEW 2026-08-09 — provider domain rename**~~ — **RESOLVED 2026-08-09** (ratified + implemented): `provider/list` / `provider/upsert` / `provider/validate` keep their names with camelCase canonical types (`ProviderVendorInfo` / `ProviderModelBindingInfo`, 1:1 conversions both ways). Dual-shape: list is `{}` on both surfaces (protocol-keyed result projection), upsert/validate sniff the camelCase `providerVendor` key — the desktop migrated without flipping surfaces. `api_key` stays write-only; `credential` stays a reference id. `model/config*`/`model/saved` resolved separately as #12.
12. ~~**NEW 2026-08-09 — `model/preferences/read|write`**~~ — **RESOLVED 2026-08-10**: canonical preferences return effective defaults (`model` = binding id or slug, `reasoning_effort`) plus flat selectable lists (`available_models`/`available_efforts` as `{value, label, description}`); permission mode deliberately excluded (session-scoped). `model/preferences/write` is patch-semantics and naturally idempotent. The legacy model/config façade is deleted and desktop uses the canonical methods.

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-08-02 | Assistant | Initial | Phase A inventory. Agent/exec domains resolved via unified task model (L2-DES-APP-008 DD-7); status Approved by human 2026-08-02. |
