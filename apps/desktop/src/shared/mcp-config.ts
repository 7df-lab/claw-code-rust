/**
 * Shared MCP config constants.
 *
 * Used by both the main process and the renderer. Keep this module
 * free of Electron or React imports so it can be bundled in either context.
 */

/**
 * Sentinel path for `open-in:open`. Main treats this as "open the user MCP
 * config file" so the renderer can use the existing `window.devo.openIn`
 * bridge when a newer `mcp.openConfig` preload method is not available yet.
 */
export const MCP_CONFIG_OPEN_PATH = "devo-mcp-config"
