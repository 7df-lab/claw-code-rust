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
			this.listeners = this.listeners.filter((candidate) => candidate !== listener)
		}
	}

	connected(): boolean {
		return true
	}

	emit(event: DevoNativeTransportEvent): void {
		for (const listener of this.listeners) listener(event)
	}
}

describe("session.summarize", () => {
	test("calls session/compact/start instead of sending /compact as a prompt", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "subscription/create") {
				return { subscriptionId: "sub-1", snapshots: [], replay: [], cursors: [] }
			}
			if (method === "session/compact/start") {
				return {
					turn: {
						id: "turn-1",
						sessionId: "session-1",
						sequence: 1,
						kind: "compaction",
						status: "inProgress",
						model: { provider: "unknown", model: "test-model" },
						startedAt: "2026-08-24T00:00:00Z",
					},
				}
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		await client.session.summarize({ sessionID: "session-1" })

		expect(transport.requests.map((request) => request.method)).toEqual([
			"initialize",
			"subscription/create",
			"session/compact/start",
		])
		expect(transport.requests.at(-1)?.params).toEqual({ sessionId: "session-1" })
		expect(transport.requests.some((request) => request.method === "turn/start")).toBe(false)
	})

	test("persists distinct start and complete compaction transcript markers", async () => {
		const session = {
			id: "session-1",
			version: 1,
			cwd: "/repo",
			title: "Compact session",
			parent: null,
			createdAt: "2026-08-24T00:00:00Z",
			lastActivityAt: "2026-08-24T00:00:00Z",
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
				updatedAt: "2026-08-24T00:00:00Z",
			},
		}
		const transport = new FakeTransport((method) => {
			if (method === "initialize") {
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			}
			if (method === "session/list") {
				return { data: [session], nextCursor: null }
			}
			if (method === "subscription/create") {
				return { subscriptionId: "sub-1", snapshots: [], replay: [], cursors: [] }
			}
			if (method === "session/resume") {
				return { session, lastContextOccupancy: null }
			}
			if (method === "session/messages/list" || method === "session/items/list") {
				return { data: [], nextCursor: null }
			}
			if (method === "session/queue/list") {
				return { entries: [] }
			}
			if (method === "context/usage/read") {
				return { occupancy: null }
			}
			throw new Error(`unexpected method ${method}`)
		})

		const client = createDevoClient({ directory: "/repo", transport })
		await client.event.subscribe()

		transport.emit({
			type: "notification",
			method: "context/compactionStarted",
			params: {
				sessionId: "session-1",
				turnId: "turn-compact",
				trigger: "manual",
			},
		})
		transport.emit({
			type: "notification",
			method: "item/started",
			params: {
				item: {
					id: "item-compact-1",
					sessionId: "session-1",
					turnId: "turn-compact",
					seq: 1,
					revision: 1,
					createdAt: "2026-08-24T00:00:00.000Z",
					updatedAt: "2026-08-24T00:00:00.000Z",
					state: "running",
					item: {
						type: "contextCompaction",
						trigger: "manual",
						summary: "Compaction started",
					},
				},
			},
		})
		transport.emit({
			type: "notification",
			method: "item/completed",
			params: {
				item: {
					id: "item-compact-1",
					sessionId: "session-1",
					turnId: "turn-compact",
					seq: 1,
					revision: 2,
					createdAt: "2026-08-24T00:00:00.000Z",
					updatedAt: "2026-08-24T00:00:02.000Z",
					state: "completed",
					item: {
						type: "contextCompaction",
						trigger: "manual",
						summary: "Context compacted",
					},
				},
			},
		})

		const { data } = await client.session.messages({ sessionID: "session-1" })
		const texts = data.flatMap((entry) =>
			entry.parts
				.filter((part) => part.type === "text")
				.map((part) => ({
					id: entry.info.id,
					text: part.text,
					status: (part as { metadata?: Record<string, unknown> }).metadata?.[
						"devo/compactionStatus"
					],
				})),
		)

		expect(texts).toEqual([
			{
				id: "compaction-item-compact-1-started",
				text: "Compacting context",
				status: "started",
			},
			{
				id: "compaction-item-compact-1-completed",
				text: "Context compacted",
				status: "completed",
			},
		])
	})
})
