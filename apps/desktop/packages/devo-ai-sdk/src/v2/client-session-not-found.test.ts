import { describe, expect, test } from "bun:test"
import {
	createDevoClient,
	isSessionNotFoundError,
	type DevoNativeTransport,
	type DevoNativeTransportEvent,
} from "./client"

class FakeTransport implements DevoNativeTransport {
	readonly requests: Array<{ method: string; params: unknown }> = []

	constructor(private readonly handler: (method: string, params: unknown) => unknown) {}

	async request(method: string, params?: unknown): Promise<unknown> {
		this.requests.push({ method, params })
		return this.handler(method, params)
	}

	async respond(): Promise<void> {}

	subscribe(_listener: (event: DevoNativeTransportEvent) => void): () => void {
		return () => {}
	}

	connected(): boolean {
		return true
	}
}

const nativeSession = {
	id: "missing-session",
	version: 1,
	cwd: "/repo",
	title: "Missing",
	parent: null,
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

describe("isSessionNotFoundError", () => {
	test("matches server message and SessionNotFound code", () => {
		expect(isSessionNotFoundError(new Error("session does not exist"))).toBe(true)
		const coded = new Error("gone") as Error & { code?: string }
		coded.code = "SessionNotFound"
		expect(isSessionNotFoundError(coded)).toBe(true)
		expect(isSessionNotFoundError(new Error("timeout"))).toBe(false)
	})
})

describe("session.messages soft-handles missing sessions", () => {
	test("returns empty messages and emits session.deleted when resume fails", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [nativeSession], nextOffset: null }
			}
			if (method === "subscription/create") {
				return { subscriptionId: "sub-1", snapshots: [], replay: [], cursors: [] }
			}
			if (method === "session/resume") {
				const error = new Error("session does not exist") as Error & { code?: string }
				error.code = "SessionNotFound"
				throw error
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const deletedIds: string[] = []
		const subscription = await client.event.subscribe()
		const consumer = (async () => {
			for await (const globalEvent of subscription.stream) {
				if (globalEvent.payload?.type === "session.deleted") {
					deletedIds.push(String(globalEvent.payload.properties?.info?.id ?? ""))
					break
				}
			}
		})()

		const result = await client.session.messages({ sessionID: "missing-session" })
		expect(result.data).toEqual([])
		await consumer
		expect(deletedIds).toEqual(["missing-session"])
		expect(transport.requests.some((request) => request.method === "session/resume")).toBe(true)
	})
})

describe("session.queue.list resumes cold historical sessions", () => {
	test("loads via session/resume before queue/list", async () => {
		let resumed = false
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [nativeSession], nextOffset: null }
			}
			if (method === "session/resume") {
				resumed = true
				return { session: nativeSession }
			}
			if (method === "session/items/list") {
				return { data: [], nextCursor: null }
			}
			if (method === "session/queue/list") {
				if (!resumed) {
					const error = new Error("session does not exist") as Error & { code?: string }
					error.code = "SessionNotFound"
					throw error
				}
				return {
					entries: [
						{
							queueItemId: "q1",
							position: 0,
							preview: "hello",
							input: [{ type: "text", text: "hello" }],
							enqueuedAt: "2026-01-01T00:00:00.000Z",
						},
					],
				}
			}
			if (method === "subscription/create") {
				return { subscriptionId: "sub-1", snapshots: [], replay: [], cursors: [] }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const result = await client.session.queue.list({ sessionID: "missing-session" })
		expect(resumed).toBe(true)
		expect(result.data.entries).toHaveLength(1)
		expect(result.data.entries[0]?.queueItemId).toBe("q1")
		const methods = transport.requests.map((request) => request.method)
		expect(methods.indexOf("session/resume")).toBeLessThan(methods.indexOf("session/queue/list"))
	})

	test("returns empty entries when the session is truly gone", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [nativeSession], nextOffset: null }
			}
			if (method === "session/resume") {
				const error = new Error("session does not exist") as Error & { code?: string }
				error.code = "SessionNotFound"
				throw error
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const result = await client.session.queue.list({ sessionID: "missing-session" })
		expect(result.data.entries).toEqual([])
	})
})
