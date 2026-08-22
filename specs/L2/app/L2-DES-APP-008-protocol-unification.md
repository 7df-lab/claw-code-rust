---
artifact_id: L2-DES-APP-008
revision: 4
status: Approved
active_baseline: yes
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-08-22
---

# L2-DES-APP-008 — Protocol Unification: Canonical Core with Edge Adapters

## Purpose

Converge the program's three coexisting protocol surfaces — legacy flat RPC, the canonical protocol, and the ACP adapter — onto a single server-side protocol, and establish the adapter pattern by which external protocols (ACP today, A2A and others in the future) integrate without becoming peers of the core protocol. Refines L1-REQ-APP-001.

## Background / Context

Three surfaces coexist today:

1. **Legacy-shaped handlers** (shrinking flat-RPC compatibility surface): `command/exec`, the turn/input fallbacks, and historical compatibility aliases. Canonical settings, goals, search, agent, rollback, and session metadata routes are now the first-party surfaces.
2. **Canonical protocol** (61 methods defined, `crates/protocol/src/native/`): `session/*`, `turn/*`, `subscription/*`, `task/*`, `session/goal/*`, `skill/*`, `SessionSettings`, `expected_version` optimistic concurrency, typed items. Structurally a superset; partially wired on the server.
3. **ACP adapter** (`crates/server/src/runtime/handlers/acp*.rs`): serves external ACP clients.

The event path is inverted: every wire notification is projected into an ACP `session/update` envelope (`acp_notification_from_server_event`), and the native `ServerEvent` is smuggled inside the envelope's `_meta.devo.original_event` for first-party clients to unwrap. Consequences observed in production: the same event can reach a client twice through different surfaces; every client processes two event shapes; settings-like concepts exist in two divergent type families (e.g. `SessionMetadataUpdateParams` twice).

Decision (human-approved 2026-08-02): **the canonical devo-native protocol is the single retained surface.** ACP semantics are a true subset of the canonical model, so canonical can be the source and ACP only a projection; the inverse is impossible. During migration, ACP-client-visible behavior is frozen (no compatibility pressure from Zed or other ACP clients at present, but the freeze stands as discipline).

## Scope

Covers: request/response surface convergence, event protocol convergence, legacy deletion, the adapter pattern for ACP and future protocols (A2A), and the migration plan. Does not cover: the settings two-plane runtime semantics (see L2-DES-CONV-002), transport mechanics (see L2-DES-APP-003).

## Design Decisions

### DD-1: Canonical is the only server-side surface

All business behavior is served exclusively through canonical methods and canonical typed events. First-party clients (TUI, web, desktop) speak canonical directly — no first-party adapter layer. The legacy flat RPC surface is deleted, not maintained.

### DD-2: Adapters are pure translation, and ACP is an adapter

An adapter is a transport plus a projection: it translates external requests into canonical calls and canonical events into the external event model. Adapters contain zero business logic and hold no state that is not derivable from the canonical surface. ACP becomes the first such adapter; A2A and future protocols plug in identically. Adapter-facing behavior (ACP wire shape) is frozen for the duration of the migration.

### DD-3: One event protocol; the ACP envelope is an adapter output

Canonical typed events over `subscription/*` are the only event protocol. The `_meta.devo.original_event` smuggling, the per-connection "typed-or-ACP-wrapped" fallback in `notification_for`, and first-party unwrapping code (`original_event_from_acp_notification` consumers in `devo-client` and the TUI's `acp_events.rs`) are deleted. The ACP projection of canonical events is produced only for ACP adapter connections.

### DD-4: During migration, legacy translates; it does not implement

While both surfaces are being served, a legacy method is handled by translating its params into the canonical model at the handler boundary and invoking the canonical path — never by a parallel implementation. This makes behavioral divergence between the two surfaces structurally impossible and keeps the migration safe to ship incrementally. Status (2026-08-11): the standalone settings/title routes and their flat DTOs are removed; `session/metadata/update` is canonical-only, including title, permission, sandbox, and compaction settings.

### DD-5: Settings unification rides this migration

The canonical `session/metadata/update` (`SessionSettings` patch + `expected_version`) already is the unified settings entry point anticipated by L2-DES-CONV-002 DD-10; no separate `session/settings/update` method will be invented. The settings two-plane work (L2-DES-CONV-002) lands its write path as the pilot domain of Phase B below.

### DD-6: Contract guards during migration

Two distinct guards exist, and both stay green throughout the migration:

1. **`protocol-lock.json`** pins the *external* protocol snapshots (ACP v1 schema, A2A 1.0 proto — the A2A entry already exists) with sha256 truth sources. It is the enforcement mechanism of the adapter freeze (DD-2): adapter-visible wire shapes cannot drift without a deliberate lock-file update.
2. **`crates/server/tests/protocol_contract.rs`** pins devo's *own* wire shapes via serialization roundtrip/shape tests. Phase B extends it with canonical shape tests domain by domain as each canonical handler is wired; legacy shape tests are deleted together with the legacy surface in Phase E.

### DD-7: Unified background-task abstraction for agents and exec

Sub-agents (L1-REQ-AGENT-004) and background processes (L1-REQ-TOOL-005) present the same consumer-facing shape — a unit of background work the user can list, inspect (status / output / result), and stop, with its parent relationship visible. Canonical already models both as session items addressed by `item_id`.

**Decision**: both are unified as **tasks** — background work units owned by a session, addressed by `item_id`, discriminated by `kind: "process" | "agent"`. The control surface is shared: `task/start` (kind-discriminated params), `task/list`, `task/read`, `task/write_stdin` (process stdin), `task/message` (agent natural-language steer, internally a child-session turn), `task/resize` (process pty), `task/interrupt` (the uniform stop action). Events reuse the existing item lifecycle family (`item/started` / `item/delta` / `item/completed`); no new event families. The backing stays explicit: `kind: "process"` is an OS process/pty inheriting the session sandbox; `kind: "agent"` is a child session (the task item holds the child `session_id`, preserving the existing parent/child session model and the L1 visibility requirement) with its own turns, queue, and permission scope. This resolves the agent/exec REDESIGN items in the mapping table: `agent/spawn` and `command/exec` become `task/start` variants, `agent/wait` becomes `task/read` + subscription, and a public agent-start path deliberately exists because L1-REQ-AGENT-004 requires user-requested subagent creation (superseding canonical's earlier no-public-spawn stance). Exact verb names are finalized in the Phase B domain design.

### DD-8: Interactive approvals and structured questions are server→client requests on the canonical surface

The approval engine already resolves decisions through reverse JSON-RPC, not through the session-actor mailbox: `request_permission_from_controllers` (`crates/server/src/runtime/control_requests.rs`) fans-outs the canonical approval projection to Native/canonical controllers and the ACP permission projection to ACP controllers, the first valid answer wins, and the turn awaits a oneshot in the interactive registry. That blocking model is exactly what keeps approvals responsive while `ExecuteTurn` blocks the actor (the compact deadlock lesson) and is unchanged by this decision. Structured questions now use the same canonical reverse-request pattern.

**Decision**: on native-surface connections (L2-DES-APP-009 DD-6), the server sends the canonical reverse requests: `approval/command/request` (shell exec), `approval/fileChange/request` (file write), `approval/permission/request` (other resources), and `userInput/request` for structured questions. On ACP connections, the server retains the ACP `session/request_permission` projection and ACP clients may answer it; ACP filesystem requests remain capability-gated by `clientCapabilities.fs`. The Native request params are the waiting-state item payload — canonical `Item::Approval` with `decision = None`, `Item::UserInputRequest` with `answers = None` — so the wire request, the persisted item, and the subscription event are one fact. The corresponding clients answer `ApprovalRespondParams` / `UserInputRespondParams`, while ACP outcomes translate into the same internal decision/scope tuple at the adapter boundary. `session/goal/completionApproval/request` follows the same model when the goal completion flow migrates.

For Native approvals, the reverse request is registered and queued before the
waiting item event is published, and first-party clients register the request
in receive order before dispatching that event to UI code. Interactive waits
have no wall-clock timeout; interrupt, disconnect, and session termination
complete the waiting item as cancelled and clear its correlation state.
`Item::Approval` carries optional normalized command pattern/prefix fields so
the TUI can render every offered command scope without consulting legacy state.

### DD-9: Runtime protocol exposure is a monotonic `ProtocolSet`

`ServerProtocol::{Native, Acp}` is the user-facing domain vocabulary. `Native` names the client-facing API and the `"native"` wire marker. A non-empty process-local `ProtocolSet` aggregate controls which adapters may be selected during connection initialization. The default server set is Native only; ACP must be explicitly enabled with `devo server --protocols acp` or `devo server --protocols native,acp`.

The singleton control plane may only union protocols into the running set through authenticated internal `server/protocols/enable`; it cannot disable them. A later server invocation extends the existing process before proxying stdio or reporting listener status. `--status` and `--shutdown` do not mutate the set.

Connections begin without a protocol. During `initialize`, `_meta.devo.protocol = "native"` selects Native and an absent marker selects stable ACP v1.20. The runtime rejects a disabled selection with JSON-RPC `InvalidRequest` while leaving the connection uninitialized and retryable. After initialization, dispatch, event projection, approvals, and user-input requests use only the negotiated adapter. Application events are emitted once and projected once per eligible connection; enabling another adapter changes future initialization choices only and cannot duplicate or replay events.

## Migration Phases

**Phase A — Inventory and freeze (one PR).** Produce the complete legacy→canonical mapping table (see the appendix `L2-DES-APP-008-legacy-canonical-mapping.md`: renamed counterparts, MERGE-INTO consolidations, GAPs requiring new canonical definitions, REDESIGN items requiring product decisions). Confirm the contract guards (DD-6). Rule in effect from here: new features land on canonical only; legacy only shrinks.

**Phase B — Native serving, domain by domain.** Order: session domain → turn domain → **settings domain (pilot, via Native `session/metadata/update`; merges L2-DES-CONV-002 Phase 2)** → agent/model/provider/search/workspace domains. Per domain: wire the Native handler (reusing existing application internals), convert the legacy handler into a boundary translator (DD-4), add contract tests. Status (2026-08-12): **settings domain done**; **interrupt domain done** — `session/interrupt` is the sole Native interrupt request, with scoped turn/task/command handling. `turn/interrupt` is removed from the Native surface.

**Phase C — TUI switches to canonical.** `devo-client` gains a canonical transport; the TUI migrates feature by feature (session load → turn streaming → queue → settings). Events migrate wholesale to typed items: delete `crates/tui/src/worker/acp_events.rs` and the `_meta` unwrapping in `devo-client`. Acceptance: full TUI test baseline plus manual smoke (send, steer, queue, interrupt, resume).

**Phase D — ACP adapter purification.** ACP handlers become in-process clients of the canonical surface; ACP event projection derives from the canonical event stream; `_meta` original-event embedding is removed. Produce the adapter-pattern guide (adapter = transport + projection, zero business logic) as the A2A onboarding document.

**Phase E — Legacy deletion.** Remove the remaining legacy-shaped handlers, flat param structs, `events/subscribe`, `SubscriptionFilter`, duplicate param shapes, and non-ACP uses of `acp_notification_from_server_event`. Update `protocol-lock.json` and docs.

Each phase is independently shippable. Rollback within a phase is ordinary revert; the DD-4 translation rule guarantees no client-visible behavior change until Phase E deletion.

## Risks and Mitigations

- **Dual-serving divergence**: eliminated structurally by DD-4 (translation, not parallel implementation).
- **External ACP client breakage**: ACP surface frozen; ACP contract tests must stay green throughout; no ACP-visible change until a deliberate, versioned decision post-migration.
- **TUI regression during migration**: in-repo client with a full test baseline; migrate feature-by-feature with tests, events last-but-one as a single cutover.
- **Scope creep**: the mapping table (Phase A) is the exhaustive scope list; anything not in the table is out of scope.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-APP-001 | 1 | specs/L1/L1-REQ-APP-001-client-server-arch.md | Protocol unification refines the client-server architecture requirement into a single canonical surface with edge adapters. |
| related-to | L2-DES-APP-003 | 2 | specs/L2/app/L2-DES-APP-003-client-server-protocol.md | The broader client-server protocol architecture; this document supersedes its multi-surface status quo with a unification plan. |
| related-to | L2-DES-CONV-002 | 1 | specs/L2/conv/L2-DES-CONV-002-two-plane-session-settings.md | The settings two-plane write path lands as the pilot domain of Phase B via canonical `session/metadata/update`. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-08-02 | Assistant | Initial | Initial draft; direction approved by human (canonical as single surface; ACP behavior frozen during migration). Status Approved by human 2026-08-02, including DD-7 unified task abstraction. |
| 2 | 2026-08-09 | Assistant | Added DD-8 | Canonical reverse-request model for approvals and structured questions (mixed-surface fan-out; decision vocabulary converges on canonical `ApprovalDecision`). |
| 3 | 2026-08-11 | Assistant | Added DD-9 | Runtime `ProtocolSet`, Native CLI vocabulary, monotonic singleton extension, initialize-time adapter selection, and once-per-connection event projection. |
| 4 | 2026-08-22 | Assistant | Clarified DD-8 | Defined request-before-event ordering, lifecycle cancellation cleanup, indefinite interactive waits, and command pattern/prefix carriage for Native approvals. |
