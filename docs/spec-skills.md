# Skills Detailed Specification

## Background and Goals

`devo` must support skills as reusable instruction bundles that can be discovered from disk, referenced by users, and injected into turns in a structured way.

Primary goals:

- define the skill discovery and loading model
- support skill references as explicit structured input, not only text conventions
- keep skill loading deterministic, cacheable, and observable

## Scope

In scope:

- skill discovery from configured roots
- skill metadata extraction
- skill enablement and disablement
- injecting skill content into turn input
- change detection and reload behavior

Out of scope:

- remote skill marketplace distribution
- arbitrary skill execution semantics beyond prompt/context injection

## Module Responsibilities and Boundaries

`devo-core` owns:

- normalized skill input items in conversation and prompt assembly
- resolving referenced skills into prompt-view content

`devo-server` owns:

- `skills/list`
- `skills/changed`
- skill config write APIs if exposed

`devo-utils` may own:

- filesystem watching and path normalization helpers used by skill discovery

## Core Data Structures

```rust
pub struct SkillId(pub SmolStr);

pub struct SkillRecord {
    pub id: SkillId,
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    pub enabled: bool,
    pub source: SkillSource,
}
```

```rust
pub enum SkillSource {
    User,
    Workspace { cwd: PathBuf },
    Plugin { plugin_id: String },
}
```

## Discovery and Loading

Rules:

- skills are discovered from configured roots plus workspace-specific roots
- skill identity must be stable across reloads when path and declared name are unchanged
- skill content is loaded from the canonical skill document path
- skill discovery results may be cached per workspace root
- file watching or explicit reload must invalidate stale skill caches

## Turn Integration

Rules:

- users may reference skills through structured `skill` input items
- plain-text `$skill-name` markers may be supported as a convenience, but explicit structured items are preferred
- resolved skill content is injected into the prompt as structured supporting context, not as a hidden side channel
- missing or disabled skills must produce a clear runtime-visible failure or warning rather than silent omission

## Configuration Definitions

```rust
pub struct SkillsConfig {
    pub enabled: bool,
    pub user_roots: Vec<PathBuf>,
    pub workspace_roots: Vec<PathBuf>,
    pub watch_for_changes: bool,
}
```

## Error Handling Strategy

Required error categories:

- `SkillNotFound`
- `SkillDisabled`
- `SkillParseFailed`
- `SkillRootUnavailable`

## Observability

Required logs and metrics:

- `skills.discovery.count`
- `skills.discovery.failure.count`
- `skills.reload.count`
- `skills.changed.notification.count`
- `skills.inject.count`

## Testing Strategy and Acceptance Criteria

Required tests:

- skill discovery from configured roots
- enabled and disabled filtering
- structured skill input injection
- cache invalidation on file change
- missing-skill error behavior

Acceptance criteria:

- users and clients can list and reference skills deterministically
- skill injection produces reproducible prompt-view content
