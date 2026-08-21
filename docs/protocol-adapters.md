# Protocol Adapter Guide

How to integrate an external protocol (ACP today, A2A and others in the future) with the devo server without becoming a peer of the core protocol. This is the onboarding document required by `L2-DES-APP-008` Phase D.

## The rule

**An adapter is transport plus projection. Zero business logic, zero state that is not derivable from the Native application path.**

The Native API is devo's first-party protocol and the single source of truth. Its shared types live in `crates/protocol/src/native/`; ACP and future A2A are edge adapters. Every behavior is served through the Native path first. Adapters translate; they never implement.

```
external client ──► adapter (transport + projection) ──► Native application path ──► server runtime
```

## Runtime protocol exposure

The process-local `ProtocolSet` aggregate controls which adapters a connection may select:

```bash
devo server                         # Native only
devo server --protocols acp         # ACP v1 only
devo server --protocols native,acp  # both
```

Protocol names are case-sensitive. Whitespace is trimmed and duplicate names are ignored. A later invocation against the existing singleton extends the enabled set monotonically; protocols cannot be disabled until that server restarts. `--status` and `--shutdown` never change the set.

Selection occurs exactly once during `initialize`. `_meta.devo.protocol = "native"` selects Native; an absent marker selects ACP v1. Any other marker is treated as ACP selection. A disabled selection receives `InvalidRequest` and the connection remains uninitialized, so it can retry after the singleton has been extended.

Enabling an adapter does not duplicate events. The runtime emits each application event once, visits each eligible connection once, and asks that connection's negotiated adapter for one projection. Existing connections keep their negotiated protocol and receive no replay when another adapter is enabled.

## Anatomy of an adapter

An adapter has exactly three parts:

1. **Transport** — how bytes move: method name mapping, request/response envelopes, error-code mapping. Example: ACP's JSON-RPC methods (`session/new`, `session/prompt`, `session/set_mode`, …) and ACP error codes.
2. **Request projection** — external params → Native calls. This is a *translation at the handler boundary* (L2-DES-APP-008 DD-4): the adapter invokes the same internal path the Native handler invokes, never a parallel implementation. Semantics that cannot be expressed in the external model are rejected explicitly, never silently approximated.
3. **Event projection** — Native events → external event model, produced only for adapter connections. First-party clients consume Native typed events directly; adapters never see `_meta` smuggling or dual shapes.

## What an adapter must NOT do

- Hold business state. If a value cannot be derived from the Native session/turn/item state on demand, it does not belong in the adapter.
- Add endpoints for convenience. If the Native surface lacks the concept, the answer is a Native vocabulary proposal (L2 revision), not an adapter-side invention.
- Change external wire behavior without a `protocol-lock.json` update. The lock file pins the external spec snapshot (sha256 truth sources); adapter-visible shapes are frozen by default. ACP targets stable v1.20; ACP v2 draft behavior is out of scope.
- Share code with first-party clients beyond the Native types themselves. The projection is one-directional: Native → external.

## Onboarding checklist (e.g. A2A)

1. Pin the external spec in `protocol-lock.json` (snapshot path + sha256 truth source). A2A's entry already exists.
2. Define protocol types for the external surface in `crates/protocol/src/<proto>/`, marked clearly as adapter types (they are not part of the Native model).
3. Implement request projection: external method → Native call at the handler boundary. Reject inexpressible semantics with a mapped error.
4. Implement event projection: Native `ServerNotification` → external notification shape, delivered only to that adapter's connections.
5. Add contract tests in `crates/server/tests/` pinning the external wire shape (params/results/events) against the lock-file snapshot version.
6. Document unsupported semantics explicitly in the adapter's module docs.

## Reference implementation

The ACP adapter (`crates/server/src/runtime/handlers/acp/`, wire types and projection helpers in `crates/protocol/src/acp/`) is the current reference. It owns ACP v1 route registration, wire types, capability handling, and projections. It does not own session, turn, or business state. First-party connections do not pass through its routes or event projector.
