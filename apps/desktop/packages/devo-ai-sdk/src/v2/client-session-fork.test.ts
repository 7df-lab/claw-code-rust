import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport, type DevoNativeTransportEvent } from "./client"

class FakeTransport implements DevoNativeTransport {
	readonly requests: Array<{ method: string; params: unknown }> = []
	private listeners: Array<(event: DevoNativeTransportEvent) => void> = []

	constructor(private readonly handler: (method: string, params: unknown) => unknown) {}

	async request(method: string, params?: unknown): Promise<unknown> {
		this.requests.push({ method, params })
		return this.handler(method, params)
	}

	async respond(): Promise<void> {}

	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void {
		this.listeners.push(listener)
		return () => {
			this.listeners = this.listeners.filter((item) => item !== listener)
		}
	}

	connected(): boolean {
		return true
	}
}

const forkedSession = {
	id: "child-session",
	version: 1,
	cwd: "/repo",
	title: "Forked",
	parent: null,
	forkFromId: "parent-session",
	atTurnId: "turn-2",
	createdAt: "2026-01-01T00:00:00.000Z",
	lastActivityAt: "2026-01-01T00:00:00.000Z",
	status: "idle",
	flags: [],
	archived: false,
	ephemeral: false,
	model: { provider: "test", model: "test-model" },
	settings: { permissionProfile: "default" },
	preview: "",
	queuedCount: 0,
	usage: {
		total: {
			inputTokens: 0,
			outputTokens: 0,
			cacheCreationInputTokens: 0,
			cacheReadInputTokens: 0,
			reasoningTokens: 0,
			totalTokens: 0,
			callCount: 0,
			meteredCallCount: 0,
			failedCallCount: 0,
			cancelledCallCount: 0,
		},
		byPurpose: [],
		updatedAt: "2026-01-01T00:00:00.000Z",
	},
}

describe("session.fork", () => {
	test("sends canonical session/fork params and remembers the child", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") {
				return {
					protocolVersion: 1,
					agentCapabilities: {},
					authMethods: [],
				}
			}
			if (method === "session/fork") {
				expect(params).toEqual({
					sessionId: "parent-session",
					atTurnId: "turn-2",
					cut: "before",
				})
				return { session: forkedSession }
			}
			if (method === "session/resume") {
				return { session: forkedSession }
			}
			if (method === "session/items/list") {
				return { data: [], nextCursor: null }
			}
			if (method === "subscription/create") {
				return { subscriptionId: "sub-1", cursors: [] }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({
			directory: "/repo",
			transport,
		})

		const result = await client.session.fork({
			sessionID: "parent-session",
			atTurnId: "turn-2",
			cut: "before",
		})

		expect(result.data.id).toBe("child-session")
		expect(result.data.forkFromId).toBe("parent-session")
		expect(result.data.parentID).toBeUndefined()
		expect(transport.requests.some((request) => request.method === "session/fork")).toBe(true)
	})
})
