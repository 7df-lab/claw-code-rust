import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport, type DevoNativeTransportEvent } from "./client"

class FakeTransport implements DevoNativeTransport {
	readonly requests: Array<{ method: string; params: unknown }> = []

	constructor(private readonly handler: (method: string, params: unknown) => unknown) {}

	async request(method: string, params?: unknown): Promise<unknown> {
		this.requests.push({ method, params })
		return this.handler(method, params)
	}

	async respond(): Promise<void> {}

	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void {
		void listener
		return () => {}
	}

	connected(): boolean {
		return true
	}
}

const nativeSession = {
	id: "session-1",
	version: 1,
	cwd: "/repo",
	title: "Repro",
	parent: null,
	forkFromId: null,
	atTurnId: null,
	createdAt: "2026-08-30T07:00:00.000Z",
	lastActivityAt: "2026-08-30T07:00:00.000Z",
	status: "idle",
	flags: [],
	archived: false,
	ephemeral: false,
	model: { provider: "unknown", model: "test-model", reasoningEffort: "high" },
	settings: { permissionProfile: "default", mode: "plan", reasoningEffort: "high" },
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
		updatedAt: "2026-08-30T07:00:00.000Z",
	},
}

function envelope(method: string, eventId: string): Record<string, unknown> {
	return {
		event: {
			eventId,
			streamId: "session:session-1",
			seq: 1,
			emittedAt: "2026-08-30T07:00:00.000Z",
			persisted: true,
			schemaVersion: 1,
		},
		notification: { method, params: { session: nativeSession } },
	}
}

/**
 * Regression for the Desktop "subscription/create against
 * SubscriptionCreateResult: /replay/N/notification/method must be equal to
 * one of the allowed values" loop: one replay envelope whose notification
 * method this schema build does not know must be skipped (replay processing
 * ignores unknown methods anyway), never fail the whole subscription.
 */
describe("subscription replay forward compatibility", () => {
	test("event.subscribe survives an unknown replay notification method", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [nativeSession], nextCursor: null }
			}
			if (method === "subscription/create") {
				return {
					subscriptionId: "sub_01a051ccae687f12891cc843a70224d2",
					cursors: [{ streamId: "session:session-1", seq: 3 }],
					snapshots: [
						{
							streamId: "session:session-1",
							barrierSeq: 3,
							data: { kind: "sessionsList", sessions: [nativeSession] },
						},
					],
					replay: [
						envelope("session/created", "e1"),
						// The offending shape: a persisted event whose method this
						// build's ServerNotification schema does not know.
						envelope("session/title/updated", "e2"),
						envelope("session/metadataUpdated", "e3"),
					],
				}
			}
			if (method === "subscription/ack") {
				return { serverTimeMs: 0 }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const result = await client.event.subscribe()

		expect(typeof result.stream[Symbol.asyncIterator] === "function" || result.stream !== undefined).toBe(true)
		const created = transport.requests.find((request) => request.method === "subscription/create")
		expect(created).toBeDefined()
	})

	test("normalized sessions keep persisted model and settings for composer re-seeding", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [nativeSession], nextCursor: null }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const result = await client.session.list()
		const session = (result.data as Array<Record<string, unknown>>)[0]
		expect(session.model).toEqual({
			provider: "unknown",
			model: "test-model",
			reasoningEffort: "high",
		})
		expect(session.settings).toEqual({
			mode: "plan",
			reasoningEffort: "high",
			permissionProfile: "default",
		})
	})

	test("updateSettings persists a combined selection patch in one metadata/update", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/metadata/update") {
				expect(params).toEqual({
					sessionId: "session-1",
					expectedVersion: 0,
					model: { provider: "", model: "test-model" },
					settings: { reasoningEffort: "high", mode: "plan" },
				})
				return { session: nativeSession }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		await client.session.updateSettings({
			sessionID: "session-1",
			modelID: "test-model",
			reasoningEffort: "high",
			mode: "plan",
		})
		expect(
			transport.requests.some((request) => request.method === "session/metadata/update"),
		).toBe(true)

		// An empty patch must not hit the wire at all.
		const before = transport.requests.length
		await client.session.updateSettings({ sessionID: "session-1" })
		expect(transport.requests.length).toBe(before)
	})

	test("updateSettings writes a cold session without resuming it", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/metadata/update") {
				expect(params).toMatchObject({ sessionId: "session-1" })
				return { session: nativeSession }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		await client.session.updateSettings({ sessionID: "session-1", modelID: "test-model" })
		const calls = transport.requests.map((request) => request.method)
		expect(calls).not.toContain("session/resume")
		expect(calls.filter((method) => method === "session/metadata/update").length).toBe(1)
	})

	test("updateSettings serializes in-flight changes and preserves the latest patch", async () => {
		let releaseFirst: (() => void) | undefined
		const firstRequest = new Promise<void>((resolve) => {
			releaseFirst = resolve
		})
		const transport = new FakeTransport(async (method, params) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/metadata/update") {
				const request = params as { settings?: Record<string, string> }
				if (!request.settings?.mode) await firstRequest
				return {
					session: {
						...nativeSession,
						settings: { ...nativeSession.settings, ...(request.settings ?? {}) },
					},
				}
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		const first = client.session.updateSettings({ sessionID: "session-1", modelID: "model-a" })
		await Promise.resolve()
		const second = client.session.updateSettings({ sessionID: "session-1", mode: "plan" })
		releaseFirst?.()
		await Promise.all([first, second])

		const updates = transport.requests.filter((request) => request.method === "session/metadata/update")
		expect(updates).toHaveLength(2)
		expect(updates[1]?.params).toMatchObject({ settings: { mode: "plan" } })
		expect("model" in (updates[1]?.params ?? {})).toBe(false)
	})

	test("retrySettings replays a failed patch retained by the queue", async () => {
		let failed = true
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/metadata/update") {
				if (failed) throw new Error('{"code":"InvalidParams","message":"rejected"}')
				return { session: nativeSession }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		await expect(
			client.session.updateSettings({ sessionID: "session-1", modelID: "test-model" }),
		).rejects.toThrow("rejected")
		failed = false
		await client.session.retrySettings({ sessionID: "session-1" })
		expect(
			transport.requests.filter((request) => request.method === "session/metadata/update"),
		).toHaveLength(2)
	})
})
