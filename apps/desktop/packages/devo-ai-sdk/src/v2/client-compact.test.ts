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
})
