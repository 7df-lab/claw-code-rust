import { afterAll, describe, expect, test } from "bun:test"
import { MCP_CONFIG_OPEN_PATH } from "../../shared/mcp-config"

type OpenInCall = { directory: string; targetId: string }

const openCalls: OpenInCall[] = []
const originalWindow = globalThis.window

const fakeDevo = {
	mcp: undefined as { openConfig: () => Promise<{ path: string }> } | undefined,
	openIn: {
		openMcpConfig: undefined as (() => Promise<{ path: string }>) | undefined,
		getTargets: async () => ({
			targets: [],
			availableTargets: ["cursor"],
			preferredTarget: "cursor",
		}),
		open: async (directory: string, targetId: string) => {
			openCalls.push({ directory, targetId })
		},
		setPreferred: async () => ({ success: true }),
	},
}

Object.defineProperty(globalThis, "window", {
	configurable: true,
	value: { devo: fakeDevo },
})

const { openMcpConfigFile } = await import("./backend")

describe("openMcpConfigFile", () => {
	test("uses the existing openIn bridge when mcp.openConfig is missing", async () => {
		openCalls.length = 0
		fakeDevo.mcp = undefined
		fakeDevo.openIn.openMcpConfig = undefined
		await openMcpConfigFile()
		expect(openCalls).toEqual([{ directory: MCP_CONFIG_OPEN_PATH, targetId: "cursor" }])
	})

	test("prefers mcp.openConfig when the preload method exists", async () => {
		openCalls.length = 0
		fakeDevo.mcp = {
			openConfig: async () => ({ path: "/home/me/.devo/config.toml" }),
		}
		expect(await openMcpConfigFile()).toEqual({ path: "/home/me/.devo/config.toml" })
		expect(openCalls).toEqual([])
	})
})

afterAll(() => {
	if (originalWindow === undefined) {
		Reflect.deleteProperty(globalThis, "window")
	} else {
		Object.defineProperty(globalThis, "window", {
			configurable: true,
			value: originalWindow,
		})
	}
})
