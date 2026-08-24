import { afterEach, beforeEach, describe, expect, mock, test } from "bun:test"
import type { DevoClient } from "@devo-ai/sdk/v2/client"
import type { Event, Session } from "../lib/types"
import { partsFamily, partStorageKey } from "../atoms/parts"
import { projectPaginationFamily, sessionFamily, upsertSessionAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"

class FakeEventStream {
	private readonly queue: Array<{ payload: Event }> = []
	private readonly waiters: Array<(result: IteratorResult<{ payload: Event }>) => void> = []
	private closed = false

	push(payload: Event): void {
		const item = { payload }
		const waiter = this.waiters.shift()
		if (waiter) {
			waiter({ value: item, done: false })
			return
		}
		this.queue.push(item)
	}

	close(): void {
		this.closed = true
		for (const waiter of this.waiters.splice(0)) {
			waiter({ value: undefined, done: true })
		}
	}

	async *[Symbol.asyncIterator](): AsyncIterableIterator<{ payload: Event }> {
		while (true) {
			const queued = this.queue.shift()
			if (queued) {
				yield queued
				continue
			}
			if (this.closed) return
			const next = await new Promise<IteratorResult<{ payload: Event }>>((resolve) => {
				this.waiters.push(resolve)
			})
			if (next.done) return
			yield next.value
		}
	}
}

const streams = new Map<string, FakeEventStream>()
let activeManager: typeof import("./connection-manager") | null = null
let listSessionsImpl: (client: DevoClient, options?: unknown) => Promise<Session[]> = async () => []
let getSessionStatusesImpl: () => Promise<Record<string, { type: string }>> = async () => ({})

function streamFor(directory: string): FakeEventStream {
	let stream = streams.get(directory)
	if (!stream) {
		stream = new FakeEventStream()
		streams.set(directory, stream)
	}
	return stream
}

mock.module("./devo", () => ({
	connectToServer: (_url: string, options?: { directory?: string }) =>
		({ directory: options?.directory ?? "__base__" }) as unknown as DevoClient,
	disposeAllInstances: () => {},
	getSession: async () => null,
	getSessionStatuses: async () => getSessionStatusesImpl(),
	listProjects: async () => [],
	projectsFromSessions: (sessions: Session[]) => sessions,
	listSessions: async (client: DevoClient, options?: unknown) => listSessionsImpl(client, options),
	subscribeToGlobalEvents: async (client: DevoClient) =>
		streamFor(((client as unknown as { directory?: string }).directory) ?? "__base__"),
}))

describe("connection manager project event bridge", () => {
	beforeEach(() => {
		streams.clear()
		listSessionsImpl = async () => []
		getSessionStatusesImpl = async () => ({})
		;(globalThis as unknown as { window: Record<string, unknown> }).window = {}
		;(globalThis as unknown as { requestAnimationFrame: (callback: FrameRequestCallback) => number }).requestAnimationFrame =
			(callback) => setTimeout(() => callback(performance.now()), 0) as unknown as number
		;(globalThis as unknown as { cancelAnimationFrame: (id: number) => void }).cancelAnimationFrame = (id) =>
			clearTimeout(id)
	})

	afterEach(async () => {
		activeManager?.disconnect()
		activeManager = null
		for (const stream of streams.values()) {
			stream.close()
		}
		delete (globalThis as unknown as { window?: unknown }).window
		delete (globalThis as unknown as { requestAnimationFrame?: unknown }).requestAnimationFrame
		delete (globalThis as unknown as { cancelAnimationFrame?: unknown }).cancelAnimationFrame
	})

	test("forwards project-scoped session status events into renderer state", async () => {
		const directory = "/repo/project-status-bridge"
		const session: Session = {
			id: "status-bridge-session",
			directory,
			title: "Status bridge",
			time: { created: 1, updated: 1 },
		}
		appStore.set(upsertSessionAtom, { session, directory })

		const manager = await import(`./connection-manager?case=${Date.now()}`)
		activeManager = manager
		await manager.connectToDevo("devo://stdio")
		expect(manager.getProjectClient(directory)).not.toBeNull()

		streamFor(directory).push({
			type: "session.status",
			properties: {
				sessionID: session.id,
				status: { type: "busy" },
			},
		})
		await new Promise((resolve) => setTimeout(resolve, 5))

		expect(appStore.get(sessionFamily(session.id))?.status).toEqual({ type: "busy" })
	})

	test("forwards project-scoped message part events into renderer state", async () => {
		const directory = "/repo/project-message-bridge"
		const session: Session = {
			id: "message-bridge-session",
			directory,
			title: "Message bridge",
			time: { created: 1, updated: 1 },
		}
		appStore.set(upsertSessionAtom, { session, directory })

		const manager = await import(`./connection-manager?case=${Date.now()}`)
		activeManager = manager
		await manager.connectToDevo("devo://stdio")
		expect(manager.getProjectClient(directory)).not.toBeNull()

		streamFor(directory).push({
			type: "message.part.updated",
			properties: {
				part: {
					id: "assistant-message-text",
					sessionID: session.id,
					messageID: "assistant-message",
					type: "text",
					text: "hello from project stream",
				},
			},
		})
		await new Promise((resolve) => setTimeout(resolve, 5))

		expect(appStore.get(partsFamily(partStorageKey(session.id, "assistant-message")))).toEqual([
			{
				id: "assistant-message-text",
				sessionID: session.id,
				messageID: "assistant-message",
				type: "text",
				text: "hello from project stream",
			},
		])
	})

	test("loads project statuses after session list hydration", async () => {
		const directory = "/repo/list-status-hydration"
		const session: Session = {
			id: "status-after-list-session",
			directory,
			title: "Status after list",
			time: { created: 1, updated: 1 },
		}
		let listHydrated = false
		listSessionsImpl = async () => {
			await new Promise((resolve) => setTimeout(resolve, 0))
			listHydrated = true
			return [session]
		}
		getSessionStatusesImpl = async () =>
			listHydrated ? { [session.id]: { type: "busy" } } : {}

		const manager = await import(`./connection-manager?case=${Date.now()}`)
		activeManager = manager
		await manager.connectToDevo("devo://stdio")
		await manager.loadProjectSessions(directory)

		expect(appStore.get(sessionFamily(session.id))?.status).toEqual({ type: "busy" })
	})

	test("refills the current project page after a visible session is deleted", async () => {
		const directory = "/repo/delete-refill"
		const sessions: Session[] = Array.from({ length: 6 }, (_, index) => ({
			id: `delete-refill-${index + 1}`,
			directory,
			title: `Session ${index + 1}`,
			time: { created: 6 - index, updated: 6 - index },
		}))
		const deletedIds = new Set<string>()
		listSessionsImpl = async (_client, options) => {
			const remaining = sessions.filter((session) => !deletedIds.has(session.id))
			const limit = (options as { limit?: number } | undefined)?.limit
			return limit === undefined ? remaining : remaining.slice(0, limit)
		}

		const manager = await import(`./connection-manager?case=${Date.now()}`)
		activeManager = manager
		await manager.connectToDevo("devo://stdio")
		await manager.loadAllProjects()
		await manager.loadProjectSessions(directory, undefined, { limit: 5, roots: true })

		expect(appStore.get(sessionFamily(sessions[5].id))).toBeNull()

		deletedIds.add(sessions[0].id)
		await manager.refillProjectSessionsAfterDelete(directory, sessions[0].id)

		expect({
			deleted: appStore.get(sessionFamily(sessions[0].id)),
			refilled: appStore.get(sessionFamily(sessions[5].id))?.session,
			pagination: appStore.get(projectPaginationFamily(directory)),
		}).toEqual({
			deleted: null,
			refilled: sessions[5],
			pagination: {
				loaded: true,
				currentLimit: 5,
				hasMore: false,
				loading: false,
			},
		})
	})

	test("loads another project's sessions even when discovery only returned one project", async () => {
		const projectA = "/repo/alpha"
		const projectB = "/repo/beta"
		const sessionA: Session = {
			id: "alpha-session",
			directory: projectA,
			title: "Alpha",
			time: { created: 2, updated: 2 },
		}
		const sessionB: Session = {
			id: "beta-session",
			directory: projectB,
			title: "Beta",
			time: { created: 1, updated: 1 },
		}
		listSessionsImpl = async (client) => {
			const directory = (client as { directory?: string }).directory
			if (directory === "__base__") return [sessionA]
			if (directory === projectB) return [sessionB]
			if (directory === projectA) return [sessionA]
			return []
		}

		const manager = await import(`./connection-manager?case=${Date.now()}`)
		activeManager = manager
		await manager.connectToDevo("devo://stdio")
		await manager.loadAllProjects()
		await manager.loadProjectSessions(projectB, undefined, { limit: 5, roots: true })

		expect({
			otherProject: appStore.get(sessionFamily(sessionB.id))?.session,
			discoveryOnly: appStore.get(sessionFamily(sessionA.id)),
		}).toEqual({
			otherProject: sessionB,
			discoveryOnly: null,
		})
	})
})
