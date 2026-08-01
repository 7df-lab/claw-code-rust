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
- Transport: `stdio` with `command = ["devo-code-search-mcp"]`
- Workspace root: stdio process cwd (Devo launches with session workspace as
  fallback cwd when the server record omits `cwd`)
- Binary is shipped next to `devo` in CLI archives, install scripts, and desktop
  runtime `bin/`

## Enablement

Users enable with `devo mcp enable code_search` or TUI `/mcps`. Config load
ensure-by-id inserts the bundled record when missing and never overwrites a
user record with the same id.

## Traceability

Refines MCP integration architecture and replaces the former
`[experimental] code-search` built-in tool gate.
