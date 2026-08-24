import path from "node:path"
import { describe, expect, test } from "bun:test"
import {
	StdioNativeClient,
	buildServerProcessEnv,
	requestTimeoutMsForMethod,
	routeNativeLine,
} from "./native-stdio-client"

describe("routeNativeLine", () => {
	test("routes JSON-RPC responses, notifications, and server requests", () => {
		const responseMessage = { jsonrpc: "2.0", id: 7, result: { ok: true } }
		const response = routeNativeLine(JSON.stringify(responseMessage))
		expect(response).toEqual({
			type: "response",
			id: 7,
			message: responseMessage,
		})

		const notificationMessage = {
			jsonrpc: "2.0",
			method: "item/started",
			params: { item: { id: "item-1", sessionId: "s1" } },
		}
		const notification = routeNativeLine(JSON.stringify(notificationMessage))
		expect(notification).toEqual({
			type: "notification",
			method: "item/started",
			params: { item: { id: "item-1", sessionId: "s1" } },
			message: notificationMessage,
		})

		const requestMessage = {
			jsonrpc: "2.0",
			id: 9,
			method: "userInput/request",
			params: { requestId: "request-1", questions: [] },
		}
		const request = routeNativeLine(JSON.stringify(requestMessage))
		expect(request).toEqual({
			type: "request",
			id: 9,
			method: "userInput/request",
			params: { requestId: "request-1", questions: [] },
			message: requestMessage,
		})
	})
})

describe("StdioNativeClient", () => {
	test("builds server env with bin dir first while preserving caller env", () => {
		const env = buildServerProcessEnv({
			baseEnv: { PATH: "/usr/bin", KEEP: "base" },
			homeDir: "/Users/tester",
			optionsEnv: { CUSTOM_FLAG: "1", PATH: "/custom/bin" },
			pathSeparator: ":",
		})

		expect(env).toMatchObject({
			CUSTOM_FLAG: "1",
			KEEP: "base",
			PATH: `${path.join("/Users/tester", ".devo", "bin")}:/custom/bin`,
		})
	})

	test("uses bundled runtime bin dir first when provided", () => {
		const env = buildServerProcessEnv({
			baseEnv: { PATH: "/usr/bin", KEEP: "base" },
			homeDir: "/Users/tester",
			pathSeparator: ":",
			runtimeBinDir: "/Applications/Devo.app/Contents/Resources/runtime/bin",
		})

		expect(env).toMatchObject({
			KEEP: "base",
			PATH: "/Applications/Devo.app/Contents/Resources/runtime/bin:/usr/bin",
		})
	})

	test("adds desktop custom proxy environment for the managed runtime", () => {
		const env = buildServerProcessEnv({
			baseEnv: { PATH: "/usr/bin" },
			homeDir: "/Users/tester",
			pathSeparator: ":",
			networkProxy: {
				mode: "custom",
				proxyUrl: "socks5h://127.0.0.1:7890",
				noProxy: "localhost,127.0.0.1,::1",
			},
		})

		expect(env).toMatchObject({
			DEVO_DESKTOP_NETWORK_PROXY_MODE: "custom",
			DEVO_DESKTOP_NETWORK_PROXY_URL: "socks5h://127.0.0.1:7890",
			DEVO_DESKTOP_NETWORK_NO_PROXY: "localhost,127.0.0.1,::1",
			HTTP_PROXY: "socks5h://127.0.0.1:7890",
			HTTPS_PROXY: "socks5h://127.0.0.1:7890",
			ALL_PROXY: "socks5h://127.0.0.1:7890",
			NO_PROXY: "localhost,127.0.0.1,::1",
		})
	})

	test("clears inherited proxy environment when desktop proxy mode is off", () => {
		const env = buildServerProcessEnv({
			baseEnv: {
				PATH: "/usr/bin",
				HTTP_PROXY: "http://proxy.example:8080",
				HTTPS_PROXY: "http://proxy.example:8080",
				ALL_PROXY: "http://proxy.example:8080",
				NO_PROXY: "localhost",
				http_proxy: "http://proxy.example:8080",
				https_proxy: "http://proxy.example:8080",
				all_proxy: "http://proxy.example:8080",
				no_proxy: "localhost",
			},
			homeDir: "/Users/tester",
			pathSeparator: ":",
			networkProxy: {
				mode: "off",
				proxyUrl: "",
				noProxy: "localhost,127.0.0.1,::1",
			},
		})

		expect(env).toMatchObject({
			DEVO_DESKTOP_NETWORK_PROXY_MODE: "off",
		})
		expect(env).not.toHaveProperty("HTTP_PROXY")
		expect(env).not.toHaveProperty("HTTPS_PROXY")
		expect(env).not.toHaveProperty("ALL_PROXY")
		expect(env).not.toHaveProperty("NO_PROXY")
		expect(env).not.toHaveProperty("http_proxy")
		expect(env).not.toHaveProperty("https_proxy")
		expect(env).not.toHaveProperty("all_proxy")
		expect(env).not.toHaveProperty("no_proxy")
	})

	test("preserves inherited proxy environment when desktop proxy mode is system", () => {
		const env = buildServerProcessEnv({
			baseEnv: {
				PATH: "/usr/bin",
				HTTP_PROXY: "http://proxy.example:8080",
			},
			homeDir: "/Users/tester",
			pathSeparator: ":",
			networkProxy: {
				mode: "system",
				proxyUrl: "",
				noProxy: "localhost,127.0.0.1,::1",
			},
		})

		expect(env).toMatchObject({
			HTTP_PROXY: "http://proxy.example:8080",
			DEVO_DESKTOP_NETWORK_PROXY_MODE: "system",
		})
	})

	test("rejects and clears pending requests when stdin write fails", async () => {
		const client = new StdioNativeClient()
		const epipe = Object.assign(new Error("write EPIPE"), { code: "EPIPE" })
		;(client as unknown as { child: unknown }).child = {
			killed: false,
			pid: 123,
			stdin: {
				destroyed: false,
				writable: true,
				writableEnded: false,
				write: () => {
					throw epipe
				},
			},
		}

		await expect(client.request("initialize")).rejects.toThrow("write EPIPE")
		expect((client as unknown as { pending: Map<unknown, unknown> }).pending.size).toBe(0)
		expect(client.connected()).toBe(false)
	})

	test("gives MCP admin RPCs a longer timeout than ordinary requests", () => {
		expect({
			sessionList: requestTimeoutMsForMethod("session/list", 10_000),
			mcpTools: requestTimeoutMsForMethod("mcp/tools", 10_000),
			mcpSetEnabled: requestTimeoutMsForMethod("mcp/set_enabled", 5),
		}).toEqual({
			sessionList: 10_000,
			mcpTools: 60_000,
			mcpSetEnabled: 60_000,
		})
	})

	test("times out ordinary RPCs with the method and request id", async () => {
		const client = new StdioNativeClient({ requestTimeoutMs: 5 })
		;(client as unknown as { child: unknown }).child = {
			killed: false,
			pid: 123,
			stdin: {
				destroyed: false,
				writable: true,
				writableEnded: false,
				write: (_line: string, callback: (error?: Error) => void) => {
					callback()
					return true
				},
			},
		}

		await expect(client.request("session/list", {})).rejects.toThrow(
			"session/list request 1 timed out",
		)
		expect((client as unknown as { pending: Map<unknown, unknown> }).pending.size).toBe(0)
	})

	test("records outgoing requests and incoming responses with raw payloads", async () => {
		const records: unknown[] = []
		const client = new StdioNativeClient({
			trafficLogger: {
				getState: () => ({ enabled: true, path: null }),
				record: (entry) => records.push(entry),
			},
		})
		;(client as unknown as { child: unknown }).child = {
			killed: false,
			pid: 123,
			stdin: {
				destroyed: false,
				writable: true,
				writableEnded: false,
				write: (_line: string, callback: (error?: Error) => void) => {
					callback()
					return true
				},
			},
		}

		const response = client.request("initialize", { protocolVersion: 1 })
		await Promise.resolve()
		;(client as unknown as { handleLine: (line: string) => void }).handleLine(
			JSON.stringify({ jsonrpc: "2.0", id: 1, result: { ok: true } }),
		)

		await expect(response).resolves.toEqual({ ok: true })
		expect(records).toEqual([
			{
				direction: "desktop-to-server",
				kind: "request",
				id: 1,
				method: "initialize",
				payload: {
					jsonrpc: "2.0",
					id: 1,
					method: "initialize",
					params: { protocolVersion: 1 },
				},
			},
			{
				direction: "server-to-desktop",
				kind: "response",
				id: 1,
				method: "initialize",
				payload: { jsonrpc: "2.0", id: 1, result: { ok: true } },
			},
		])
	})

	test("reuses the first initialize result for later initialize calls", async () => {
		const records: unknown[] = []
		let writeCount = 0
		const client = new StdioNativeClient({
			trafficLogger: {
				getState: () => ({ enabled: true, path: null }),
				record: (entry) => records.push(entry),
			},
		})
		;(client as unknown as { child: unknown }).child = {
			killed: false,
			pid: 123,
			stdin: {
				destroyed: false,
				writable: true,
				writableEnded: false,
				write: (_line: string, callback: (error?: Error) => void) => {
					writeCount += 1
					callback()
					return true
				},
			},
		}

		const first = client.request("initialize", { protocolVersion: 1 })
		await Promise.resolve()
		;(client as unknown as { handleLine: (line: string) => void }).handleLine(
			JSON.stringify({ jsonrpc: "2.0", id: 1, result: { ok: true } }),
		)
		await expect(first).resolves.toEqual({ ok: true })

		const second = client.request("initialize", { protocolVersion: 1 })
		await expect(second).resolves.toEqual({ ok: true })

		expect(writeCount).toBe(1)
		expect(
			records.filter(
				(entry) =>
					(entry as { kind?: string; method?: string }).kind === "request" &&
					(entry as { method?: string }).method === "initialize",
			),
		).toHaveLength(1)
	})

	test("scopes outgoing requests with the project directory", async () => {
		const records: unknown[] = []
		const client = new StdioNativeClient({
			trafficLogger: {
				getState: () => ({ enabled: true, path: null }),
				record: (entry) => records.push(entry),
			},
		})
		;(client as unknown as { child: unknown }).child = {
			killed: false,
			pid: 123,
			stdin: {
				destroyed: false,
				writable: true,
				writableEnded: false,
				write: (_line: string, callback: (error?: Error) => void) => {
					callback()
					return true
				},
			},
		}

		const response = client.request("session/list", {}, "/repo/project")
		await Promise.resolve()
		;(client as unknown as { handleLine: (line: string) => void }).handleLine(
			JSON.stringify({ jsonrpc: "2.0", id: 1, result: { sessions: [] } }),
		)

		await expect(response).resolves.toEqual({ sessions: [] })
		expect(records[0]).toMatchObject({
			direction: "desktop-to-server",
			kind: "request",
			method: "session/list",
			payload: {
				params: { cwd: "/repo/project" },
			},
		})
	})

	test("does not add project directory to unscoped requests", async () => {
		const records: unknown[] = []
		const client = new StdioNativeClient({
			trafficLogger: {
				getState: () => ({ enabled: true, path: null }),
				record: (entry) => records.push(entry),
			},
		})
		;(client as unknown as { child: unknown }).child = {
			killed: false,
			pid: 123,
			stdin: {
				destroyed: false,
				writable: true,
				writableEnded: false,
				write: (_line: string, callback: (error?: Error) => void) => {
					callback()
					return true
				},
			},
		}

		const response = client.request("turn/start", { sessionId: "s1", input: [] }, "/repo/project")
		await Promise.resolve()
		;(client as unknown as { handleLine: (line: string) => void }).handleLine(
			JSON.stringify({ jsonrpc: "2.0", id: 1, result: { stopReason: "end_turn" } }),
		)

		await expect(response).resolves.toEqual({ stopReason: "end_turn" })
		expect(records[0]).toMatchObject({
			direction: "desktop-to-server",
			kind: "request",
			method: "turn/start",
			payload: {
				params: { sessionId: "s1", input: [] },
			},
		})
	})

	test("records server requests, notifications, invalid lines, and closed events", () => {
		const records: unknown[] = []
		const client = new StdioNativeClient({
			trafficLogger: {
				getState: () => ({ enabled: true, path: null }),
				record: (entry) => records.push(entry),
			},
		})
		const handleLine = (client as unknown as { handleLine: (line: string) => void }).handleLine.bind(
			client,
		)

		handleLine(
			JSON.stringify({
				jsonrpc: "2.0",
				id: "server-1",
				method: "userInput/request",
				params: { requestId: "request-1", questions: [] },
			}),
		)
		handleLine(
			JSON.stringify({
				jsonrpc: "2.0",
				method: "session/statusChanged",
				params: { sessionId: "s1", status: "idle", flags: [], activeTurnId: null },
			}),
		)
		handleLine("{not json")
		client.stop()

		expect(records).toEqual([
			{
				direction: "server-to-desktop",
				kind: "request",
				id: "server-1",
				method: "userInput/request",
				payload: {
					jsonrpc: "2.0",
					id: "server-1",
					method: "userInput/request",
					params: { requestId: "request-1", questions: [] },
				},
			},
			{
				direction: "server-to-desktop",
				kind: "notification",
				method: "session/statusChanged",
				payload: {
					jsonrpc: "2.0",
					method: "session/statusChanged",
					params: { sessionId: "s1", status: "idle", flags: [], activeTurnId: null },
				},
			},
			{
				direction: "system",
				kind: "invalid",
				payload: {
					error: "JSON Parse error: Expected '}'",
					line: "{not json",
				},
			},
			{
				direction: "system",
				kind: "closed",
				payload: { error: "Devo Native stdio client stopped" },
			},
		])
	})
})
