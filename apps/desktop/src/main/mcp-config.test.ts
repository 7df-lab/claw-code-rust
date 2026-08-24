import { afterAll, describe, expect, mock, test } from "bun:test"
import { existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

mock.module("electron", () => ({
	shell: {
		openPath: async () => "",
	},
}))

mock.module("./settings-store", () => ({
	getSettings: () => ({ openIn: { preferredTargetId: null } }),
	updateSettings: () => ({}),
}))

mock.module("./open-in-targets", () => ({
	getOpenInTargets: async () => ({
		targets: [],
		availableTargets: [],
		preferredTarget: null,
	}),
	openInTarget: async () => ({ success: true }),
}))

const { ensureUserMcpConfigFile, userMcpConfigPath } = await import("./mcp-config")
const { MCP_CONFIG_OPEN_PATH } = await import("../shared/mcp-config")

function withTempDir(run: (dir: string) => void): void {
	const dir = mkdtempSync(join(tmpdir(), "devo-mcp-config-"))
	try {
		run(dir)
	} finally {
		rmSync(dir, { recursive: true, force: true })
	}
}

describe("user MCP config file", () => {
	test("resolves config.toml under the Devo home directory", () => {
		expect(userMcpConfigPath(join("home", ".devo"))).toBe(join("home", ".devo", "config.toml"))
	})

	test("creates a stub config.toml when the file is missing", () => {
		withTempDir((dir) => {
			const configPath = userMcpConfigPath(dir)
			ensureUserMcpConfigFile(configPath)
			expect(existsSync(configPath)).toBe(true)
			const body = readFileSync(configPath, "utf-8")
			expect(body).toContain("[mcp_servers.<id>]")
			expect(body).toContain('command = "npx"')
		})
	})

	test("does not overwrite an existing config.toml", () => {
		withTempDir((dir) => {
			const configPath = userMcpConfigPath(dir)
			writeFileSync(configPath, "theme = \"aurora\"\n", "utf-8")
			ensureUserMcpConfigFile(configPath)
			expect(readFileSync(configPath, "utf-8")).toBe("theme = \"aurora\"\n")
		})
	})

	test("uses a stable sentinel path for the existing openIn preload bridge", () => {
		expect(MCP_CONFIG_OPEN_PATH).toBe("devo-mcp-config")
	})
})

afterAll(() => {
	mock.restore()
})
