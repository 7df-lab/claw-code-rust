/**
 * User-level MCP config file (`~/.devo/config.toml`) helpers.
 * The Customize MCPs "+" action opens this file in the preferred editor.
 */

import { existsSync, mkdirSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { shell } from "electron"
import { findDevoHome } from "./native-traffic-log"
import { getOpenInTargets, openInTarget } from "./open-in-targets"

export const MCP_CONFIG_FILE_NAME = "config.toml"

const MCP_CONFIG_STUB = `\
# Devo user configuration
# Add MCP servers under [mcp_servers.<id>]. Example:
#
# [mcp_servers.time]
# command = "npx"
# args = ["-y", "mcp-server-time"]
#

`

export function userMcpConfigPath(devoHome: string): string {
	return join(devoHome, MCP_CONFIG_FILE_NAME)
}

export function ensureUserMcpConfigFile(configPath: string): void {
	const parent = dirname(configPath)
	mkdirSync(parent, { recursive: true })
	if (!existsSync(configPath)) {
		writeFileSync(configPath, MCP_CONFIG_STUB, "utf-8")
	}
}

/**
 * Ensures the user config file exists, then opens it with the General
 * settings "Default open destination" app (fallback: OS file handler).
 */
export async function openUserMcpConfigFile(): Promise<{ path: string }> {
	const configPath = userMcpConfigPath(findDevoHome())
	ensureUserMcpConfigFile(configPath)

	const { preferredTarget } = await getOpenInTargets()
	if (preferredTarget) {
		await openInTarget(configPath, preferredTarget)
	} else {
		const error = await shell.openPath(configPath)
		if (error) throw new Error(error)
	}

	return { path: configPath }
}
