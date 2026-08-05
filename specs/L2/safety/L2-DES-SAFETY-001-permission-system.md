---
artifact_id: L2-DES-SAFETY-001
revision: 1
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-25
---

# L2-DES-SAFETY-001 — Permission System Architecture

## Purpose

Define the permission model that governs what the agent is allowed to access — filesystem reads, filesystem writes, and network access — across the session lifecycle. This document establishes the architecture for permission profiles, filesystem access control, network access control, and the configuration surface.

## Scope

This document covers:
- Permission profile concept and lifecycle
- Built-in profiles and their semantics
- Custom profiles in config TOML
- Filesystem access model (special paths, access modes, deny patterns, protected metadata)
- Network access model
- Additional permissions overlay (per-call grants)
- Interaction with operating modes (Security Mode, etc.)

This document does **not** cover:
- When approval is needed (see L2-DES-SAFETY-002)
- How approval is solicited and cached (see L2-DES-SAFETY-002)
- Tool output safety / secret redaction (see L1-REQ-TOOL-001 and its future L2)
- Sandbox implementation details (deferred to L3)

## Design Decisions

### DD-1: Separate "what" from "when"

Permission profile (what the agent can access) and approval policy (when the agent must ask) are independent concerns. A profile defines the boundary; an approval policy defines the gates. Mixing them leads to combinatorial explosion in configuration and unclear user intent.

**Decision**: Permission profile and approval policy are separate configuration dimensions. A session always has both, and changing one does not implicitly change the other.

### DD-2: Built-in profiles with configurable extensions

Three built-in profiles cover the vast majority of use cases. Custom named profiles provide precise control for advanced users who need fine-grained filesystem or network rules.

**Decision**: Three built-in profiles (`:read-only`, `:workspace`, `:danger-full-access`) ship with the program. Named custom profiles are defined in user config under `[permissions.<name>]` and extend or replace built-in semantics.

### DD-3: Filesystem model uses special paths, access modes, and deny patterns

A flat list of entries is the right primitive because it composes cleanly with precedence rules (most specific wins, `none` beats `write`, `write` beats `read`). Special paths (`:root`, `:workspace_roots`, `:tmpdir`, `:minimal`) keep configuration portable across machines.

**Decision**: The filesystem policy is a list of `(path, access_mode)` entries where path can be an absolute path, a special path token, or a glob pattern (for deny-read rules).

### DD-4: Protected metadata directories are always read-only under writable roots

Directories like `.git` and `.devo` inside writable roots must never be modified by the agent without an explicit, intentional configuration override. Implicit writable-root grants must not cascade into those directories.

**Decision**: When resolving writable roots, protected metadata paths (`.git`, `.devo`, `.agents`) are automatically added as read-only carveouts unless the user has explicitly listed them as writable.

### DD-5: Network model is initially coarse

Network access starts as a binary toggle (enabled/restricted) with optional domain-level allow/deny patterns. More granular controls (per-port, per-protocol) are deferred.

## Architecture

### Permission Profile

A `PermissionProfile` is the runtime representation of what the agent may access. It is resolved once per session and may be updated during the session via explicit user action (e.g. `/permissions` command) or per-command via additional permission grants.

```
+--------------------------+
|    PermissionProfile     |
+--------------------------+
| - filesystem_policy      |
| - network_policy         |
+--------------------------+
```

#### Profile Lifecycle

1. **Configuration load**: The active profile name is resolved from config (`default_permissions`), CLI flags, or session initialization parameters.
2. **Resolution**: The named profile (built-in or custom) is compiled into a runtime `PermissionProfile` with concrete filesystem entries and network policy.
3. **Materialization**: Symbolic entries (`:workspace_roots`) are materialized against the active workspace root(s).
4. **Runtime enforcement**: The profile is evaluated at each tool call to determine whether the requested action is permitted.
5. **Mutation**: Profiles can be changed per-session (via `/permissions`) or extended per-command (via additional permissions).

### Built-in Profiles

#### `:read-only`

| Dimension | Policy |
|-----------|--------|
| Filesystem read | Full disk read access |
| Filesystem write | No write access |
| Network | Restricted |

Default when no workspace is trusted. The agent can inspect the system but cannot modify anything.

#### `:workspace`

| Dimension | Policy |
|-----------|--------|
| Filesystem read | Full disk read access |
| Filesystem write | Write access within workspace roots and temp directories |
| Network | Restricted |

Default when a workspace is trusted. The agent can read everything and write within the project boundary.

#### `:danger-full-access`

| Dimension | Policy |
|-----------|--------|
| Filesystem read | Full disk read access |
| Filesystem write | Full disk write access |
| Network | Enabled |

No sandbox restrictions. Intended for fully trusted environments or when the user explicitly opts in.

### Custom Profiles

Custom profiles are defined in `config.toml` under the `[permissions]` table. Each profile specifies filesystem entries and network settings.

```toml
[permissions.dev-profile]
network.enabled = true

[permissions.dev-profile.filesystem]
":root" = "read"
":workspace_roots" = "write"
":tmpdir" = "write"
"/var/run/docker.sock" = "none"
"*.env" = "none"
```

#### Profile Names

- Names starting with `:` are reserved for built-in profiles.
- Custom profile names are non-empty, lowercase, and use kebab-case.
- Profiles may support an optional `extends` field in the future for inheritance.

### Filesystem Access Model

#### Access Modes

| Mode | Meaning |
|------|---------|
| `read` | Can read files and list directories under the path |
| `write` | Can read, create, modify, and delete files under the path |
| `none` | Explicitly denied — even if a broader rule would allow access |

Precedence when multiple entries match a path:
1. Most specific path wins
2. At equal specificity: `none` > `write` > `read`

#### Special Paths

| Token | Resolves to |
|-------|-------------|
| `:root` | The filesystem root (`/` on Unix, drive roots on Windows) |
| `:workspace_roots` | The active workspace root(s). With subpath: `:workspace_roots/.config` |
| `:tmpdir` | The system temporary directory (`$TMPDIR` or `/tmp`) |
| `:minimal` | Platform-default readable paths (for environments where full disk read is not desired) |
| `~/...` | Tilde-expanded to the user's home directory |
| Absolute path | Used as-is |

Unknown `:prefix` tokens degrade gracefully with a warning rather than causing config load failure.

#### Deny Patterns

Entries with access `none` support glob patterns for deny-read rules:

```toml
":workspace_roots" = { "*.env" = "none", "**/secrets/**" = "none" }
```

- Glob `none` entries under `:workspace_roots` are symbolic until workspace roots are materialized.
- Non-macOS platforms may require `glob_scan_max_depth` to bound `**` expansion.
- Malformed deny globs cause the policy to fail closed (deny all matching reads).

#### Protected Metadata

When a writable root is resolved, the following directories within it are automatically marked read-only unless explicitly overridden:

| Directory | Reason |
|-----------|--------|
| `.git` | Version control integrity |
| `.devo` | Project configuration and state |
| `.agents` | Agent configuration integrity |

### Network Access Model

#### Policy

| Setting | Effect |
|---------|--------|
| `enabled = false` | All network access is blocked |
| `enabled = true` | Network access is allowed, subject to domain rules |

#### Domain-Level Rules

When network is enabled, optional domain allow/deny rules provide finer control:

```toml
[network.domains]
"github.com" = "allow"
"*.internal.corp.com" = "deny"
```

- Patterns can use `*` wildcards matching one or more subdomain labels.
- Deny rules take precedence over allow rules.
- When no domain rules are configured and network is enabled, all domains are allowed.

#### Network Proxy

Network access may optionally route through a configured proxy:

```toml
[network]
proxy_url = "http://proxy.corp.com:8080"
```

Proxy configuration is part of the profile's network settings but does not independently enable network access — `network.enabled` must also be `true`.

### Additional Permissions (Per-Call Overlay)

The agent can request additional permissions for a single command invocation without changing the session-level profile. This is the "additional permissions" escalation path.

An `AdditionalPermissionProfile` is a partial overlay that may include:
- `network.enabled`: enable network for this call
- `file_system.read`: additional readable paths
- `file_system.write`: additional writable paths

The additional permissions are merged with the active profile for the duration of one command. The merge uses the same precedence rules as the base policy (most specific wins). Additional permissions that overlap with explicit deny entries in the base policy remain denied. The resulting command stays inside the OS sandbox; only `require_escalated` is allowed to disable it after authorization.

### Security Mode Interaction

When Security Mode is active, the permission system:
- Preserves its normal behavior (Security Mode does not relax permissions).
- May enforce additional constraints (e.g., requiring a controlled environment for malware execution).
- Reports missing configuration requirements rather than silently downgrading.

## Configuration Surface

### Config TOML Keys

| Key | Type | Purpose |
|-----|------|---------|
| `default_permissions` | string | Active permission profile name |
| `[permissions.<name>]` | table | Named custom profile definition |
| `[permissions.<name>.network]` | table | Network settings for the profile |
| `[permissions.<name>.filesystem]` | table | Filesystem entries for the profile |
| `[permissions.<name>.workspace_roots]` | table | Additional workspace roots |
| `network.enabled` | boolean | Network toggle (per-profile) |

### Programmatic API

The permission system exposes these primitives to the runtime:

- `resolve_permission_profile(name, cwd) -> PermissionProfile` — Compile a named profile into runtime permissions.
- `resolve_access(path, cwd) -> AccessMode` — Determine effective access for a path.
- `can_read(path, cwd) -> bool`
- `can_write(path, cwd) -> bool`
- `network_enabled() -> bool`
- `with_additional_permissions(additional) -> PermissionProfile` — Merge additional permissions.
- `materialize_workspace_roots(workspace_roots) -> PermissionProfile` — Resolve symbolic entries.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---:|---|---|---|
| refines | L1-REQ-APP-003 | 1 | specs/L1/L1-REQ-APP-003-safety.md | Defines permission profiles, modes, and access boundaries required by application safety. |
| refines | L1-REQ-SEC-001 | 1 | specs/L1/L1-REQ-SEC-001-security-mode.md | Security Mode must preserve normal permission and approval behavior. |
| related-to | L2-DES-SAFETY-002 | 1 | specs/L2/safety/L2-DES-SAFETY-002-approval-mechanism.md | Approval mechanism references and enforces permission profiles. |
| related-to | L2-DES-TUI-CMD-007 | 1 | specs/L2/tui/slash-commands/L2-DES-TUI-CMD-007-permissions.md | /permissions command controls which profile is active. |
| related-to | L2-DES-APP-002 | 2 | specs/L2/app/L2-DES-APP-002-configuration-precedence.md | Permission config participates in config layering. |
| related-to | L2-DES-APP-005 | 2 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Config TOML schema includes `[permissions]` table. |
| specified-by | L3-BEH-CORE-004 | 2 | specs/L3/core/L3-BEH-CORE-004-permission-approval.md | L3 defines permission profile resolution, access evaluation, network enforcement, and additional per-call permissions. |
| specified-by | L3-BEH-SAFETY-001 | 2 | specs/L3/safety/L3-BEH-SAFETY-001-sandbox-enforcement.md | L3 defines OS/process-boundary sandbox enforcement. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-25 | Assistant | Initial | Initial permission system architecture. |
