---
artifact_id: L2-DES-TOOL-003
revision: 2
status: Draft
active_baseline: no
supersedes:
superseded_by:
owner: Assistant
last_updated: 2026-05-26
---

# L2-DES-TOOL-003 — Deferred Tool Loading (ToolSearch)

## Purpose

Define the deferred tool loading mechanism (`ToolSearch`) — the lazy-loading system that withholds infrequently-used tool schemas from the model prompt and loads them on demand, saving significant context-window tokens without reducing available capability.

## Background / Context

The agent execution engine can expose 25+ built-in tools and an unbounded number of MCP tools. Sending the full schema of every tool to the model on every turn burns thousands of tokens, raising cost, latency, and the risk of exceeding provider context limits.

`ToolSearch` solves this by splitting tools into three groups:

- **Pre-loaded (core)**: Always included in every prompt. These are the tools the model uses most frequently on the critical path.
- **Deferred**: Listed in a `<system-reminder>` block by name and one-line description only. The model must call `ToolSearch` with `select:<name>` to load their full schemas before use.
- **Hidden**: Never sent to the model and never listed. Reserved for internal lifecycle tools gated by session mode.

The deferred set is dynamic per effective Devo configuration, MCP server availability, enabled skills, feature flags, and session mode. The model receives the exact set of deferred tools applicable to the current session and loads them incrementally as needed.

## Source Requirements

- `L1-REQ-LLM-001` requires token efficiency and stable context prefixes for provider cache friendliness.
- `L1-REQ-TOOL-002` requires a baseline set of built-in tools covering coding-agent workflows.
- `L1-REQ-TOOL-003` requires configurable web search behavior.
- `L1-REQ-TOOL-004` requires explicit parallel tool orchestration.
- `L1-REQ-TOOL-001` requires tool safety, approval, redaction, and bounded output.
- `L1-REQ-APP-003` requires permission modes, sandboxing, and explicit approval.
- `L1-REQ-AGENT-004` requires subagent delegation where enabled.
- `L1-REQ-AGENT-003` requires visible task planning with status updates.
- `L2-DES-TOOL-001` defines the built-in tool registry, categories, lifecycle, and mode gating.
- `L2-DES-TOOL-002` defines explicit `multi_tool_use` parallel orchestration.
- `L2-DES-AGENT-001` defines the execution engine that dispatches tools.
- `L2-DES-CONV-001` defines durable session records.
- `L2-DES-APP-005` defines user-scoped and workspace-scoped `config.toml` shape.

## Design Requirement

Tool schemas must not be sent to the model unless they are likely to be used on the current or subsequent turns. Infrequently-used tools are deferred — their schemas are withheld, and the model explicitly loads them via a dedicated `ToolSearch` tool before use.

The mechanism must:

1. Classify every registered tool as pre-loaded, deferred, or hidden at session start.
2. Expose only pre-loaded + already-loaded-deferred schemas in the actual prompt.
3. List deferred-but-not-yet-loaded tools in a compact `<system-reminder>` block with one-line descriptions.
4. Provide a `ToolSearch` tool the model calls to load deferred tool schemas.
5. Track which deferred tools have been loaded per session to avoid duplicate loading.
6. Support alias resolution so the model can use common alternative names.
7. Measure and report token savings from the deferred architecture.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ System Prompt                                                 │
│   [Pre-loaded tool schemas: read, find, grep, ...] │
│                                                               │
│   <system-reminder>                                           │
│   Deferred tools:                                             │
│     web_search: Performs a web search to find relevant ...    │
│     fetch_url: Fetches and summarizes URL content ...         │
│     spawn_subagent: Launches a bounded child agent ...        │
│     skill: Activates an available skill ...                   │
│     ...                                                       │
│   </system-reminder>                                          │
└─────────────────────────────────────────────────────────────┘
         │  Model reads list, calls ToolSearch("select:web_search,fetch_url")
         ▼
┌─────────────────────────────────────────────────────────────┐
│ ToolSearch Executor                                           │
│   Parses query: "select:web_search,fetch_url"                │
│   Resolves aliases through the server alias map               │
│   Validates against the session deferred tool catalog         │
│   Marks tools as loaded in the session loaded-tool set        │
│   Returns: "Loaded 2 tool(s): web_search, fetch_url"         │
└─────────────────────────────────────────────────────────────┘
         │  Schemas now available
         ▼
┌─────────────────────────────────────────────────────────────┐
│ Next Turn Prompt                                              │
│   [Pre-loaded tool schemas: read, find, grep, ...] │
│   [Loaded deferred schemas: web_search, fetch_url]            │
│   <system-reminder>                                           │
│   Deferred tools:                                             │
│     spawn_subagent: Launch a child agent ...                  │
│     skill: Activate a skill ...                               │
│     ...                                                       │
│   </system-reminder>                                          │
└─────────────────────────────────────────────────────────────┘
```

## Tool Classification

Every tool registered in the server-owned tool registry has a prompt loading policy. At session start, the deferred tool splitter partitions all available tools into three groups based on the effective Devo configuration, mode gates, feature flags, MCP availability, and the session's previously-loaded deferred set.

### Group 1: Pre-loaded (Core) — Always in Prompt

These tools are included in every model turn when registered by the effective configuration. They are the tools the model uses on virtually every decision-to-action cycle, or tools whose absence would make ordinary agent work brittle. Feature-gated tools are present or absent according to the effective configuration.

| Tool | Category | Purpose |
|---|---|---|
| `read` | File read | Read file contents and metadata, including supported attachments. |
| `find` | Search | Filename and path search backed by ripgrep. |
| `grep` | Search | High-performance content search, normally backed by ripgrep. |
| `code_search` / `mcp__code_search__code_search` | MCP | Optional semantic code retrieval via the bundled `code_search` MCP server (`devo-code-search-mcp`). Config preset is `enabled = false` by default. |
| `ls` | File read | List directory contents with optional pattern filtering. |
| `write` | File mutation | Create or overwrite files through structured content. |
| `apply_patch` | File mutation | Apply structured patches to files. |
| `shell_command` | Command execution | Execute shell commands with bounded output, timeout, approval, and background-process policy. |
| `plan` | Planning | Maintain a visible to-do list for multi-step work. |
| `approval` | Approval | Request user approval for gated actions. |
| `question` | Plan Mode question | Ask Plan Mode clarification questions where allowed. |

### Group 2: Deferred — Loaded via ToolSearch

These tools resolve to a `deferred` prompt loading policy and are withheld from the initial prompt. Their names and one-line descriptions appear in the `<system-reminder>` block. The model loads them explicitly via `ToolSearch("select:<name>")`. Once loaded, their full schemas are included in subsequent turns.

The exact set of deferred tools varies by effective Devo configuration. Representative deferred tools include:

| Tool | Category | Purpose |
|---|---|---|
| `web_search` | Web | Perform web searches with configurable provider, result count, and domain filtering. |
| `fetch_url` | Web | Fetch and summarize content from a URL where network policy allows it. |
| `skill` | Skill activation | Activate an available skill when the skill registry is enabled. |
| `spawn_subagent` | Delegation | Launch a child agent session for bounded delegated work where subagents are enabled. |
| `subagent_status` | Delegation | Inspect running child agent sessions. |
| `subagent_result` | Delegation | Retrieve a completed child agent result. |
| `multi_tool_use` | Parallel orchestration | Execute an explicit group of valid child tool calls concurrently where enabled. |
| `goal_update` | Goal status | Report verified goal completion or blockers when the goal feature is active. |
| `ToolSearch` | Internal | Load schemas for deferred tools so they can be called. |
| `mcp__*` | MCP | Tools exposed by configured MCP servers. Conditionally deferred per server configuration. |

### Group 3: Hidden — Never Sent to Model

These tools are entirely excluded from the model prompt. They are not listed in the deferred reminder and cannot be loaded via `ToolSearch`. They are accessed through internal lifecycle events or user-initiated commands.

| Hidden surface | Tool exposure | Purpose |
|---|---|---|
| Slash-command handlers | N/A | Client-initiated commands such as `/model`, `/goal`, and `/permissions` are not model tools. |
| Configuration write APIs | N/A | User-owned configuration mutation is exposed through client/server requests, not model tool calls. |
| Disabled feature tools | N/A | Tools gated by disabled feature flags are hidden rather than listed as loadable deferred tools. |
| Blocked mode tools | N/A | Tools invalid in the current session mode are hidden or reported as unavailable according to the tool registry policy. |

### Dynamic Categorization

The deferred tool splitter performs the actual split at session start:

```
split_tool_catalog(all_tools, loaded_deferred_set, effective_config, session_mode):
  exposed = []     // pre-loaded + already-loaded deferred
  core = []        // pre-loaded only
  loadedDeferred = []  // deferred and already loaded
  hidden = []      // deferred and not yet loaded

  for each tool in all_tools:
    policy = resolve_prompt_loading_policy(tool, effective_config, session_mode)

    if policy == hidden:
      continue

    if policy == deferred:
      if loadedDeferredSet.has(tool.name):
        loadedDeferred.push(tool.spec)
      else:
        hidden.push(tool)
    else:
      core.push(tool.spec)

  exposed = [...core, ...loadedDeferred]
  return { exposed, core, loadedDeferred, hidden }
```

The `exposed` array becomes the actual tool list in the model prompt. The `hidden` array drives the `<system-reminder>` deferred tools listing. The `core` array is used for token accounting.

## ToolSearch Tool Definition

```yaml
tool_name: "ToolSearch"
display_name: "ToolSearch"
description: >
  Load schemas for deferred tools so they can be called.
  Use query "select:<name>[,<name>...]" with exact tool names
  copied from the "Deferred tools:" system reminder only.
  Do not use ToolSearch for tools already present in the current
  tool list, built-in/core tools, guessed aliases, or tools
  not listed in the reminder.
execution_owner: "server"         # handled by the server-owned tool registry
visible_to_user: false            # hidden from ordinary UI tool lists
model_callable: true              # callable by the model
requires_approval: false
category: "internal"
inputSchema:
  query: string  # "select:<name>[,<name>...]"
```

`ToolSearch` itself should normally be pre-loaded. A configuration may defer it only if the provider supports a minimal bootstrap tool set that still leaves a reliable way for the model to load additional tools.

## Executor Logic

The ToolSearch executor follows a precise six-step flow:

### Step 1 — Parse Query

```
Input:  "select:fetch_url,web_search"
Regex:  /^select:(.+)$/i
Output: ["fetch_url", "web_search"]
```

If the query does not match `select:...`, the executor returns an error with the expected format.

### Step 2 — Resolve Aliases

The executor checks each requested name against the server-owned tool alias map. If a requested name is a known alias, it is replaced with the canonical model-facing tool name before lookup.

Key alias categories:

| Alias Pattern | Maps To | Examples |
|---|---|---|
| Case variants | Canonical tool name | `WebSearch` → `web_search` |
| Legacy / shorthand names | Canonical tool name | `bash` -> `shell_command`, `readfile` -> `read` |
| Kebab-case variants | Canonical tool name | `fetch-url` → `fetch_url` |
| Common synonyms | Canonical tool name | `terminal` -> `shell_command`, `runcommand` -> `shell_command` |

If no alias match is found, the raw name is used as-is for lookup. This means the model must use exact tool names as listed in the deferred reminder.

### Step 3 — Classify Each Requested Tool

Each requested tool name is looked up in the session deferred tool catalog. The lookup uses case-normalized comparison after alias resolution. Based on the result, tools are classified into four categories:

| Category | Condition | Response Template |
|---|---|---|
| **Loaded** | Found in deferred list AND not yet loaded this session | `Loaded N tool(s): Name1, Name2, ...` |
| **Already loaded** | Found in deferred list AND already loaded | `Already loaded N tool(s): Name1. Call these tools directly...` |
| **Already available** | NOT in deferred list because it is pre-loaded | `Already available N tool(s): read, apply_patch, shell_command. Call these tools directly without loading.` |
| **Not found** | Not in deferred list AND not a pre-loaded tool | `Not found: ImaginaryTool. Only request exact tool names from the Deferred tools list above.` |

### Step 4 — Mark as Loaded

For each tool in the "Loaded" category, the executor records the canonical tool name in the session loaded-tool set. This records that the tool's schema should be included in all subsequent turns.

### Step 5 — Error on Total Failure

If ALL requested tools are "not found" and zero tools were loaded or already available, the executor returns an error. This prevents the model from silently failing when it hallucinates tool names.

### Step 6 — Return Summary

The executor yields a single text result summarizing what happened:

```
Loaded 2 tool(s): fetch_url, web_search
Already loaded 1 tool(s): skill
Already available 3 tool(s): read, apply_patch, shell_command. Call these tools directly without loading.
```

The output is formatted for both model consumption and human readability in the TUI.

## Session State Management

### Session Deferred Tool Catalog

The session deferred tool catalog stores the complete deferred tool list for a session after effective configuration, feature flags, permissions, MCP availability, skills, and mode gates are applied. It is populated during prompt assembly and is the only set that `ToolSearch` may load from.

This catalog is server-owned. Clients may receive projections of tool availability for UI state, but clients do not decide which model-facing tools are loadable.

### Loaded-Tool Tracking

The loaded-tool set tracks per-session loaded state:

```
LoadedDeferredTools:
  mark_loaded(session_id, tool_name)    // record that tool was loaded
  is_loaded(session_id, tool_name)      // check if already loaded
  list_loaded(session_id)               // get all loaded tool names
```

This prevents duplicate loading and ensures the deferred tool splitter correctly includes previously-loaded deferred tools in subsequent prompts.

## Integration with Prompt Assembly

The full prompt assembly flow, executed at each turn boundary:

```
assemble_tool_prompt(all_tools, session_id, effective_config, session_mode):
  1. Get loaded deferred tools: loaded = LoadedDeferredTools.list_loaded(session_id)
  2. Split tools: {exposed, core, loadedDeferred, hidden} =
     split_tool_catalog(all_tools, loaded, effective_config, session_mode)
  3. Store hidden deferred tools in the session deferred tool catalog
  4. Build tool schemas for exposed tools
  5. Build deferred reminder text from hidden tools
  6. Compute token metrics
  7. Return { tools, hidden, loadedDeferred, toolSearchMetrics }

build_exposed_tools(preloaded, loadedDeferred):
  // Merges pre-loaded and loaded deferred tools into final prompt list
  // Adds the ToolSearch-specific instruction to the last tool
  combined = [...preloaded, ...loadedDeferred]
  // Annotate last tool with deferred reminder instruction
  combined[last].description += deferredReminderInstruction
  return { tools: combined }
```

## Token Economics

The deferred loading system tracks token usage through `toolSearchMetrics`:

| Metric | Meaning |
|---|---|
| `baselineToolSchemaTokens` | Tokens that would be consumed if ALL tool schemas were sent. |
| `exposedToolSchemaTokens` | Tokens actually consumed by pre-loaded + loaded-deferred tool schemas. |
| `deferredReminderTokens` | Tokens consumed by the `<system-reminder>` deferred tools listing. |
| `estimatedNetToolContextTokens` | `exposed + deferredReminder` — total tokens spent on tools. |
| `estimatedTokensSaved` | `baseline − exposed` — net savings from deferral. |
| `exposedToolCount` | Number of pre-loaded tools visible to the model. |
| `hiddenToolCount` | Number of deferred-but-not-yet-loaded tools. |
| `loadedDeferredToolCount` | Number of deferred tools loaded this session. |
| `mcpToolSearchEnabled` | Whether MCP deferred loading is active. |
| `toolSearchPhase` | Current phase: `pre_tool_search` or `post_tool_search`. |

### Savings Model

The savings model estimates token counts by serializing the tool schemas that would be sent in each scenario:

```
baseline = serialize(allTools)
exposed  = serialize(preloadedTools + loadedDeferredTools)
reminder = serialize(deferredReminderBlock)

netCost  = exposed + reminder
saved    = baseline - exposed
```

In a typical session with ~25 tools and ~10 MCP tools, baseline might be 8,000–12,000 tokens while exposed might be 3,000–5,000 tokens, yielding 40–60% savings on tool schemas alone.

## Alias Mapping

The alias map provides fuzzy name matching so the model's recalled tool names resolve correctly even when the exact model-facing tool name is not used. The map may contain many entries covering:

- **Case variations**: All-lowercase, PascalCase, camelCase
- **Separator variations**: Hyphens, underscores, no separators
- **Legacy names**: Historical tool names retained for backward compatibility
- **Common synonyms**: Alternative names users and models naturally produce
- **Shorthand**: Abbreviated forms

The full alias mapping table is maintained in the ToolSearch executor module and maps known aliases to canonical model-facing tool names. Examples:

| Alias | Maps To | Category |
|---|---|---|
| `web_search`, `web-search`, `WebSearch` | `web_search` | Format variants |
| `bash`, `shell`, `runcommand`, `terminal`, `exec` | `shell_command` | Synonyms |
| `readfile`, `read-file`, `cat`, `view` | `read` | Synonyms |
| `glob`, `file_search`, `file-search` | `find` | Legacy and synonyms |
| `grep_tool`, `grep-tool`, `rg`, `ripgrep`, `search_content` | `grep` | Synonyms |
| `fetch_url`, `fetch-url`, `urlfetch`, `scrape`, `curl` | `fetch_url` | Synonyms |
| `apply_patch`, `apply-patch`, `patch` | `apply_patch` | Format variants |
| `task_tool`, `subagent`, `delegate`, `sub_agent` | `spawn_subagent` | Synonyms |
| `plan`, `todowrite`, `todo-write`, `tasklist` | `plan` | Synonyms |
| `skill_tool`, `useskill`, `use_skill` | `skill` | Synonyms |
| `tool_search`, `tool-search`, `loadtool`, `load_tool` | `ToolSearch` | Synonyms |

The alias map is server-side only. The model is still instructed to use exact names from the deferred reminder, but the alias map provides a safety net for inevitable naming inconsistencies.

## Availability and Configuration

### Effective Devo Configuration

The set of deferred tools is determined from the effective configuration after merging the user-scoped and workspace-scoped `config.toml` files described by `L2-DES-APP-005`.

Configuration may choose a conservative default and then override individual tools:

- **Development default**: file inspection, file mutation, shell, approval, and plan tools pre-loaded; web, skills, subagents, MCP tools, and parallel orchestration deferred.
- **Minimal prompt footprint**: only inspection, shell, approval, and `ToolSearch` pre-loaded; most optional capabilities deferred.
- **Specialized workspace**: domain-specific MCP tools or skills pre-loaded when they are central to the project, with general optional tools deferred.

### MCP Server Configuration

MCP tools are conditionally deferred. Each MCP server can define a tool loading policy that propagates to tools from that server. When an MCP server is deferred, its tools appear in the deferred reminder after the server's available tool list is known. When loaded via `ToolSearch`, the selected MCP tool schemas become available according to the server's trust, startup, and permission policies.

### Devo Configuration File

In user or project `config.toml`:

```toml
[mcp_servers.code_search]
command = "devo-code-search-mcp"
enabled = false

[tools.deferred_loading]
enabled = true
default_policy = "defer_optional"
preloaded = ["read", "find", "grep", "ls", "write", "apply_patch", "shell_command", "plan", "approval"]
deferred = ["web_search", "fetch_url", "skill", "spawn_subagent", "multi_tool_use"]
hidden = []

[tools.deferred_loading.mcp]
default_policy = "deferred"

[tools.deferred_loading.mcp.servers.github]
policy = "deferred"
```

The exact TOML keys should be reflected in `L2-DES-APP-005` when this design becomes part of the active baseline. Credential values remain in `auth.json`; deferred loading configuration must not duplicate or expose secrets.

## Invariants

- Tool schemas are never sent to the model unless pre-loaded or explicitly loaded via `ToolSearch`.
- `ToolSearch` can only load tools listed in the session's deferred reminder — it cannot load arbitrary or hallucinated tool names.
- Once a deferred tool is loaded, its schema persists for the remainder of the session.
- Alias resolution is a safety net, not a primary interface — the model is always instructed to use exact tool names from the deferred reminder.
- The deferred reminder is regenerated each turn with only the still-unloaded tools.
- `ToolSearch` is executed by the server-owned tool registry and follows normal tool lifecycle recording.
- Clients may render ToolSearch and availability changes, but clients do not decide model tool availability.
- Pre-loaded tools cannot be "unloaded" — they are always available.
- Hidden tools cannot be loaded through `ToolSearch`.
- Deferring a tool never bypasses permission, approval, sandbox, mode, network, credential, or MCP trust policy.
- The split between pre-loaded and deferred is deterministic per effective configuration and session state.

## Traceability

| Relationship | Target ID | Target Revision | Target Path | Rationale |
|---|---|---:|---|---|
| refines | L1-REQ-LLM-001 | 1 | specs/L1/L1-REQ-LLM-001-token-efficiency.md | ToolSearch is the primary mechanism for reducing tool schema token overhead. |
| related-to | L1-REQ-TOOL-002 | 1 | specs/L1/L1-REQ-TOOL-002-tools.md | Defines which built-in tools are subject to deferral. |
| related-to | L1-REQ-TOOL-003 | 1 | specs/L1/L1-REQ-TOOL-003-web-search-configuration.md | `web_search` is a configurable deferred tool. |
| related-to | L1-REQ-TOOL-004 | 1 | specs/L1/L1-REQ-TOOL-004-parallel-tool-orchestration.md | Parallel orchestration tool may also be deferred. |
| related-to | L1-REQ-AGENT-004 | 1 | specs/L1/L1-REQ-AGENT-004-subagents.md | Subagent delegation tools may be deferred. |
| related-to | L2-DES-TOOL-001 | 1 | specs/L2/tool/L2-DES-TOOL-001-built-in-tool-system.md | The tool registry, lifecycle, and mode gating apply to all tools including deferred ones. |
| related-to | L2-DES-TOOL-002 | 1 | specs/L2/tool/L2-DES-TOOL-002-parallel-tool-orchestration.md | Parallel orchestration interacts with deferred loading. |
| related-to | L2-DES-AGENT-001 | 1 | specs/L2/agent/L2-DES-AGENT-001-execution-engine.md | The execution engine dispatches ToolSearch like any other tool. |
| related-to | L2-DES-CONV-001 | 1 | specs/L2/conv/L2-DES-CONV-001-session-jsonl-data-model.md | Session data model stores loaded deferred state per session. |
| related-to | L2-DES-APP-005 | 2 | specs/L2/app/L2-DES-APP-005-config-toml-schema.md | Deferred loading policy is durable Devo configuration. |
| specified-by | L3-BEH-TOOLS-004 | 2 | specs/L3/tools/L3-BEH-TOOLS-004-deferred-tool-loading.md | L3 defines ToolSearch classification, executor behavior, loaded-tool tracking, prompt integration, aliases, and metrics. |

## Revision Notes

| Revision | Date | Author | Change Type | Notes |
|---:|---|---|---|---|
| 1 | 2026-05-26 | Assistant | Initial | Initial deferred tool loading design, covering ToolSearch architecture, three-group classification, executor logic, session state, token economics, and alias mapping. |
| 2 | 2026-05-26 | Assistant | Revision | Removed non-Devo tool/configuration terminology and aligned deferred loading with Devo `config.toml`, server-owned tool registry, MCP configuration, and client projection boundaries. |
