# L2-DES-MCP-002 Bundled code_search MCP server

- Artifact ID: L2-DES-MCP-002
- Revision: 1
- Status: Implemented
- Active Baseline: yes

## Summary

Semantic code retrieval is provided by the bundled stdio MCP binary
`devo-code-search-mcp`, not by a native Devo tool handler.

## Design

- Server id: `code_search`
- Tool name (MCP): `code_search`
- Model-facing name when enabled: `mcp__code_search__code_search`
- Default: config entry present with `enabled = false`, `startup_policy = lazy`
- Transport: stdio with `command = "devo-code-search-mcp"`
- Workspace root: stdio process cwd (Devo launches with session workspace as
  fallback cwd when the server record omits `cwd`)
- Binary is shipped next to `devo` in CLI archives, install scripts, and desktop
  runtime `bin/`
- Startup must accept MCP `initialize` / `tools/list` before index prewarm;
  prewarm runs in the background after the stdio server is serving

## Enablement

Users enable with `devo mcp enable code_search` or TUI `/mcps`. Config load
ensure-by-id inserts the bundled record when missing and never overwrites a
user record with the same id. Enable/disable materializes the bundled record
into user `config.toml` when it was only present in the effective in-memory
config.

In a running Devo session, TUI `/mcps` Enable/Disable calls `mcp/set_enabled`,
which persists config, starts or stops only that MCP server, and swaps the live
tool registry so the next turn can see `mcp__code_search__code_search` without
restarting the process. Offline CLI `devo mcp enable|disable` writes config for
the next process start (or for a later in-session `mcp/set_enabled` apply).

## Traceability

Refines MCP integration architecture and replaces the former
`[experimental] code-search` built-in tool gate.
