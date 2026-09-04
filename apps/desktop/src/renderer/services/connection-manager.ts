import type { DevoClient } from "@devo-ai/sdk/v2/client"
import { processEvent } from "../atoms/actions/event-processor"
import { authHeaderAtom, serverConnectedAtom, serverUrlAtom } from "../atoms/connection"
import { discoveryAtom } from "../atoms/discovery"
import { batchUpsertPartsAtom } from "../atoms/parts"
import {
	SESSIONS_PAGE_SIZE,
	projectPaginationFamily,
	removeSessionAtom,
	resetProjectPaginationAtom,
	sessionFamily,
	sessionIdsAtom,
	setProjectPaginationLoadingAtom,
	setSessionsAtom,
	updateProjectPaginationAtom,
} from "../atoms/sessions"
import { appStore } from "../atoms/store"
import {
	applyStreamingDelta,
	flushStreamingParts,
	isStreamingField,
	isStreamingPartType,
	streamingVersionFamily,
	updateStreamingPart,
} from "../atoms/streaming"
import { createLogger } from "../lib/logger"
import { directoriesMatch } from "../lib/directory-path"
import type { Event, Session } from "../lib/types"
import {
	connectToServer,
	deleteSession,
	disposeAllInstances,
	getSession,
	getSessionStatuses,
	listSessions,
	projectsFromSessions,
	subscribeToGlobalEvents,
} from "./devo"

const log = createLogger("connection-manager")

// ============================================================
// Health check
// ============================================================

/**
 * Lightweight Native health probe. Devo desktop talks to the runtime through
 * Electron's preload bridge, so readiness means the main-process stdio
 * transport is connected.
 */
async function checkHealth(url: string, authHeader: string | null): Promise<boolean> {
	void url
	void authHeader
	return typeof window !== "undefined" && "devo" in window ? window.devo.native.connected() : true
}

// ============================================================
// State — single server connection + per-project clients
// ============================================================

/** The single Devo server connection */
let connection: {
	url: string
	/** Auth header for stdio compatibility (currently null). */
	authHeader: string | null
	/** Base client (no directory) — used for the Native event subscription */
	baseClient: DevoClient
	abortController: AbortController
} | null = null

/** Per-project SDK clients, keyed by directory path */
const projectClients = new Map<string, DevoClient>()

/** Unscoped `session/list` snapshot from the latest discovery pass. */
let discoveredSessions: Session[] | null = null

/** Project clients whose local event stream is bridged into the UI store. */
const projectEventBridgeDirs = new Set<string>()

/**
 * Monotonically increasing ID for event loop instances.
 */
let eventLoopGeneration = 0

/**
 * Global reference to the Native event AbortController that survives Vite HMR
 * module replacement. When HMR replaces this module, the old module's
 * `connection` variable is lost, but the old event loop keeps running
 * with an unreachable AbortController. By storing it on `window`, the
 * new module can abort the stale loop on reconnect.
 */
const NATIVE_ABORT_KEY = "__devo_native_abort__" as const

function getGlobalAbort(): AbortController | undefined {
	// biome-ignore lint/suspicious/noExplicitAny: accessing dynamic window property for Native event abort controller
	return (window as any)[NATIVE_ABORT_KEY]
}

function setGlobalAbort(controller: AbortController | null) {
	// biome-ignore lint/suspicious/noExplicitAny: accessing dynamic window property for Native event abort controller
	;(window as any)[NATIVE_ABORT_KEY] = controller
}

function clearProjectClients(): void {
	projectClients.clear()
	projectEventBridgeDirs.clear()
}

function clearDiscoverySessionCache(): void {
	discoveredSessions = null
}

function filterDiscoveredSessions(
	sessions: Session[],
	directory: string,
	options?: { limit?: number; roots?: boolean; search?: string },
): Session[] {
	let filtered = sessions.filter((session) => {
		if (!session.directory) return false
		return directoriesMatch(session.directory, directory)
	})
	if (options?.roots) {
		filtered = filtered.filter((session) => !session.parentID)
	}
	if (options?.search) {
		const query = options.search.toLowerCase()
		filtered = filtered.filter((session) =>
			(session.title ?? session.id).toLowerCase().includes(query),
		)
	}
	if (options?.limit !== undefined) {
		filtered = filtered.slice(0, options.limit)
	}
	return filtered
}

async function hydrateProjectSessionsFromCache(
	directory: string,
	sandboxDirs: Set<string> | undefined,
	options: { limit?: number; roots?: boolean; search?: string } | undefined,
): Promise<boolean> {
	// Paginated sidebar loads must go to session/list with that project's cwd.
	// The unscoped discovery snapshot is newest-first across every project, so
	// hydrating from it hides older projects and cannot refill after deletes.
	if (options?.limit !== undefined) return false
	if (!discoveredSessions) return false

	const baseClient = getBaseClient()
	if (!baseClient) return false

	const sessions = filterDiscoveredSessions(discoveredSessions, directory, options)
	const statuses = await getSessionStatuses(baseClient)
	appStore.set(setSessionsAtom, { sessions, statuses, directory, sandboxDirs })
	return true
}

// ============================================================
// Public API
// ============================================================

/**
 * Connect to an Devo server.
 * Starts Native event subscription for all-project events.
 *
 * @param url       Base URL of the Devo server
 * @param authHeader  Deprecated compatibility auth header
 */
export async function connectToDevo(url: string, authHeader?: string | null): Promise<void> {
	// Disconnect existing connection if any
	if (connection) {
		log.info("Disconnecting previous connection", { url: connection.url })
		connection.abortController.abort()
		clearProjectClients()
		clearDiscoverySessionCache()
	}

	// Also abort any stale Native event loop from a previous HMR module that we can't
	// reach through the module-level `connection` variable.
	const staleAbort = getGlobalAbort()
	if (staleAbort && !staleAbort.signal.aborted) {
		log.info("Aborting stale Native event loop from previous module")
		staleAbort.abort()
	}

	// Bump generation — any previous event loop will see it's stale and exit
	eventLoopGeneration++
	const gen = eventLoopGeneration

	const resolvedAuth = authHeader ?? null
	appStore.set(serverUrlAtom, url)
	appStore.set(authHeaderAtom, resolvedAuth)

	// Base client has no directory — used for events that cover all projects.
	const baseClient = connectToServer(url, { authHeader: resolvedAuth ?? undefined })
	const abortController = new AbortController()

	connection = { url, authHeader: resolvedAuth, baseClient, abortController }
	setGlobalAbort(abortController)

	log.info("Connecting to Devo server", { url, authenticated: !!resolvedAuth, generation: gen })

	// Ping the server to check if it's reachable before starting the event loop.
	// This sets the initial connected state accurately instead of optimistically.
	const healthy = await checkHealth(url, resolvedAuth)
	appStore.set(serverConnectedAtom, healthy)
	if (healthy) {
		log.info("Server health check passed", { url })
	} else {
		log.warn("Server health check failed, will retry via Native event loop", { url })
	}

	// Start Native event loop in the background.
	// Connected state is updated when the event stream opens or fails.
	startEventLoop(baseClient, abortController.signal, gen)
}

/**
 * List all projects known to the Devo server via the API.
 * Uses the base client (no directory scope) since project.list() is global.
 */
export async function loadAllProjects() {
	const client = getBaseClient()
	if (!client) {
		log.warn("Cannot load projects: not connected to server")
		return []
	}
	try {
		const sessions = await listSessions(client)
		discoveredSessions = sessions
		const projects = projectsFromSessions(sessions)
		log.info("Loaded projects from API", { count: projects.length })
		return projects
	} catch (err) {
		log.error("Failed to load projects from API", err)
		return []
	}
}

async function fetchProjectSessionPage(
	client: DevoClient,
	options: { limit?: number; roots?: boolean; search?: string },
): Promise<{ sessions: Session[]; hasMore: boolean }> {
	if (options.limit === undefined) {
		return { sessions: await listSessions(client, options), hasMore: false }
	}
	const fetched = await listSessions(client, { ...options, limit: options.limit + 1 })
	const hasMore = fetched.length > options.limit
	return {
		sessions: hasMore ? fetched.slice(0, options.limit) : fetched,
		hasMore,
	}
}

/**
 * Load sessions for a specific project directory from the server.
 * Merges them into the Jotai store.
 *
 * @param directory    The project's main worktree directory
 * @param sandboxDirs  Known sandbox (worktree) directories for this project,
 *                     used to restore worktree metadata on sessions after reload
 * @param options      Optional filtering/pagination for session list
 */
export async function loadProjectSessions(
	directory: string,
	sandboxDirs?: Set<string>,
	options?: { limit?: number; roots?: boolean; search?: string },
): Promise<void> {
	if (options?.limit) {
		appStore.set(setProjectPaginationLoadingAtom, directory)
	}

	if (await hydrateProjectSessionsFromCache(directory, sandboxDirs, options)) {
		log.info("Loaded sessions for project from discovery cache", {
			directory,
			limit: options?.limit,
			roots: options?.roots,
		})
		return
	}

	const client = getProjectClient(directory)
	if (!client) return

	try {
		const { sessions, hasMore } = await fetchProjectSessionPage(client, options ?? {})
		const statuses = await getSessionStatuses(client)
		log.info("Loaded sessions for project", {
			directory,
			count: sessions.length,
			limit: options?.limit,
			roots: options?.roots,
		})
		appStore.set(setSessionsAtom, { sessions, statuses, directory, sandboxDirs })

		if (options?.limit) {
			appStore.set(updateProjectPaginationAtom, {
				directory,
				fetchedCount: sessions.length,
				limit: options.limit,
				hasMore,
			})
		}
	} catch (err) {
		log.error("Failed to load sessions", { directory }, err)
		// Reset loading state on error
		if (options?.limit) {
			appStore.set(updateProjectPaginationAtom, {
				directory,
				fetchedCount: 0,
				limit: options.limit,
			})
		}
	}
}

/**
 * Load more sessions for a project by increasing the fetch limit.
 * Called when the user clicks "Load more" in the sidebar.
 *
 * @param directory    The project's main worktree directory
 * @param currentLimit The current limit (will be increased by SESSIONS_PAGE_SIZE)
 */
export async function loadMoreProjectSessions(
	directory: string,
	currentLimit: number,
): Promise<void> {
	const nextLimit = currentLimit + SESSIONS_PAGE_SIZE
	log.info("Loading more sessions", { directory, currentLimit, nextLimit })
	appStore.set(setProjectPaginationLoadingAtom, directory)

	if (
		await hydrateProjectSessionsFromCache(directory, undefined, {
			limit: nextLimit,
			roots: true,
		})
	) {
		log.info("Loaded more sessions for project from discovery cache", {
			directory,
			limit: nextLimit,
		})
		return
	}

	const client = getProjectClient(directory)
	if (!client) {
		log.warn("Cannot load more sessions: no client for directory", { directory })
		return
	}

	try {
		const { sessions, hasMore } = await fetchProjectSessionPage(client, {
			limit: nextLimit,
			roots: true,
		})
		const statuses = await getSessionStatuses(client)
		log.info("Loaded more sessions for project", {
			directory,
			count: sessions.length,
			limit: nextLimit,
		})
		appStore.set(setSessionsAtom, { sessions, statuses, directory })
		appStore.set(updateProjectPaginationAtom, {
			directory,
			fetchedCount: sessions.length,
			limit: nextLimit,
			hasMore,
		})
	} catch (err) {
		log.error("Failed to load more sessions", { directory }, err)
		// Reset loading state on error
		appStore.set(updateProjectPaginationAtom, {
			directory,
			fetchedCount: currentLimit,
			limit: currentLimit,
		})
	}
}

/**
 * Remove a confirmed deletion from the discovery cache and refill the project's
 * existing pagination window. Keeping the same limit replaces the deleted row
 * without implicitly advancing a full page.
 */
export async function refillProjectSessionsAfterDelete(
	projectDirectory: string,
	sessionId: string,
): Promise<void> {
	if (discoveredSessions) {
		discoveredSessions = discoveredSessions.filter((session) => session.id !== sessionId)
	}
	appStore.set(removeSessionAtom, sessionId)

	const pagination = appStore.get(projectPaginationFamily(projectDirectory))
	if (!pagination.loaded) return

	try {
		await loadProjectSessions(projectDirectory, undefined, {
			limit: pagination.currentLimit,
			roots: true,
		})
	} catch (error) {
		// The server-side deletion already succeeded. Keep the deletion result
		// authoritative and allow a later discovery refresh to retry the refill.
		log.warn("Failed to refill project sessions after deletion", {
			projectDirectory,
			sessionId,
			error,
		})
	}
}

/**
 * Permanently delete every listed session for a project directory.
 * Used when removing a folder from Devo Desktop; the folder on disk is left untouched.
 */
export async function deleteProjectSessions(projectDirectory: string): Promise<void> {
	const client = getProjectClient(projectDirectory)
	if (!client) throw new Error("Not connected to Devo server")

	const sessions = await listSessions(client, { roots: true })
	log.info("Deleting all sessions for project", {
		directory: projectDirectory,
		count: sessions.length,
	})

	for (const session of sessions) {
		await deleteSession(client, session.id)
	}

	if (discoveredSessions) {
		const deletedIds = new Set(sessions.map((session) => session.id))
		discoveredSessions = discoveredSessions.filter((session) => {
			if (deletedIds.has(session.id)) return false
			return !session.directory || !directoriesMatch(session.directory, projectDirectory)
		})
	}

	for (const sessionId of [...appStore.get(sessionIdsAtom)]) {
		const entry = appStore.get(sessionFamily(sessionId))
		if (entry && directoriesMatch(entry.directory, projectDirectory)) {
			appStore.set(removeSessionAtom, sessionId)
		}
	}

	const discovery = appStore.get(discoveryAtom)
	appStore.set(discoveryAtom, {
		...discovery,
		projects: discovery.projects.filter((project) => {
			if (!project.worktree) return true
			return !directoriesMatch(project.worktree, projectDirectory)
		}),
	})

	appStore.set(resetProjectPaginationAtom, [projectDirectory])
}

/**
 * Get or create a project-scoped SDK client.
 *
 * If the module-level connection was lost (e.g. Vite HMR wiped it) but
 * the Jotai store still knows the server URL, we transparently reconnect.
 */
export function getProjectClient(directory: string): DevoClient | null {
	if (!connection) {
		// HMR recovery: module state is gone but the store remembers the URL
		const storeUrl = appStore.get(serverUrlAtom)
		if (storeUrl) {
			log.warn("Connection lost (likely HMR), reconnecting to", { url: storeUrl })

			// Abort any stale Native event loop from the previous module
			const staleAbort = getGlobalAbort()
			if (staleAbort && !staleAbort.signal.aborted) {
				log.info("Aborting stale Native event connection from previous module")
				staleAbort.abort()
			}

			const storeAuth = appStore.get(authHeaderAtom)
			const baseClient = connectToServer(storeUrl, { authHeader: storeAuth ?? undefined })
			const abortController = new AbortController()
			eventLoopGeneration++
			connection = { url: storeUrl, authHeader: storeAuth, baseClient, abortController }
			setGlobalAbort(abortController)
			startEventLoop(baseClient, abortController.signal, eventLoopGeneration)
			// Connected state is set by startEventLoop once Native event stream opens
		} else {
			return null
		}
	}

	let client = projectClients.get(directory)
	if (!client) {
		client = connectToServer(connection.url, {
			directory,
			authHeader: connection.authHeader ?? undefined,
		})
		projectClients.set(directory, client)
		startProjectEventBridge(client, directory, connection.abortController.signal)
	}
	return client
}

/**
 * Fetch a single session by ID using the global (non-directory-scoped) client.
 *
 * Used as a fallback when navigating directly to a session that is not yet in
 * the Jotai store — for example, subagent sessions that arrived while the Native
 * event stream was reconnecting, or sessions absent from the initial batch load.
 *
 * Returns `null` if the session is not found, the server is unreachable, or
 * no connection has been established.
 */
export async function fetchSessionById(sessionId: string): Promise<import("../lib/types").Session | null> {
	const client = getBaseClient()
	if (!client) return null
	return getSession(client, sessionId)
}

/**
 * Get the base SDK client (no directory scope).
 * Used for global operations like auth set/remove, provider list, global config.
 * Returns null if not connected.
 */
export function getBaseClient(): DevoClient | null {
	if (!connection) {
		// HMR recovery
		const storeUrl = appStore.get(serverUrlAtom)
		if (storeUrl) {
			const storeAuth = appStore.get(authHeaderAtom)
			const baseClient = connectToServer(storeUrl, { authHeader: storeAuth ?? undefined })
			const abortController = new AbortController()
			eventLoopGeneration++
			connection = { url: storeUrl, authHeader: storeAuth, baseClient, abortController }
			setGlobalAbort(abortController)
			startEventLoop(baseClient, abortController.signal, eventLoopGeneration)
			// Connected state is set by startEventLoop once Native event stream opens
		} else {
			return null
		}
	}
	return connection.baseClient
}

/**
 * Clear cached Native model preferences on every active SDK client.
 * Provider updates mutate server config, so model/preferences/read must be reloaded.
 */
export function invalidateConfigOptionCaches(): void {
	const clients = new Set<DevoClient>()
	if (connection?.baseClient) {
		clients.add(connection.baseClient)
	}
	for (const client of projectClients.values()) {
		clients.add(client)
	}
	for (const client of clients) {
		client.invalidateConfigOptionCaches?.()
	}
}

/**
 * Check if we're connected to the Devo server.
 */
export function isConnected(): boolean {
	return connection !== null
}

/**
 * Get the server URL, or null if not connected.
 */
export function getServerUrl(): string | null {
	return connection?.url ?? null
}

/**
 * Reload all Devo configuration by disposing all server instances.
 * This forces the server to re-read config files, agents, skills, commands, etc.
 * The resulting Native events automatically invalidate UI queries.
 */
export async function reloadConfig(): Promise<void> {
	if (!connection) {
		log.warn("Cannot reload config: not connected to server")
		return
	}
	log.info("Reloading Devo config (disposing all instances)")
	await disposeAllInstances(connection.baseClient)
}

/**
 * Disconnect from the Devo server.
 */
export function disconnect(): void {
	log.info("Disconnecting from Devo server")
	if (connection) {
		connection.abortController.abort()
		connection = null
		clearProjectClients()
	}
	setGlobalAbort(null)
	eventLoopGeneration++
	appStore.set(serverConnectedAtom, false)
}

// ============================================================
// Event Batching (Devo-inspired 16ms flush with coalescing)
// ============================================================

const FRAME_BUDGET_MS = 16

function coalescingKey(event: Event): string | undefined {
	switch (event.type) {
		case "message.part.updated": {
			const part = event.properties.part
			return `part:${part.messageID}:${part.id}`
		}
		case "message.part.delta":
			return `part:${event.properties.messageID}:${event.properties.partID}`
		case "session.status":
			return `status:${event.properties.sessionID}`
		case "context.usage.updated":
			return `context-usage:${event.properties.sessionID}`
		default:
			return undefined
	}
}

function createEventBatcher() {
	let queue: Event[] = []
	const coalesced = new Map<string, Event>()
	let scheduled: number | undefined
	let lastFlush = 0

	function flush() {
		const events = [...queue, ...coalesced.values()]
		queue = []
		coalesced.clear()
		scheduled = undefined
		lastFlush = performance.now()

		if (events.length === 0) return

		// Collect non-streaming parts across all events in the batch so we can
		// write them in a single batchUpsertPartsAtom call instead of N individual
		// upsertPartAtom calls. This significantly reduces Jotai atom writes and
		// React reconciliation passes during heavy tool-call activity.
		const batchedParts: import("../lib/types").Part[] = []
		const batchedPartSessionIds = new Set<string>()

		for (const event of events) {
			if (event.type === "session.deleted") {
				const deletedId = event.properties.info?.id
				if (deletedId && discoveredSessions) {
					discoveredSessions = discoveredSessions.filter((session) => session.id !== deletedId)
				}
			}
			if (event.type === "message.part.updated" && !isStreamingPartType(event.properties.part)) {
				batchedParts.push(event.properties.part)
				batchedPartSessionIds.add(event.properties.part.sessionID)
			} else {
				processEvent(event)
			}
		}

		// Flush collected non-streaming parts in a single batch write
		if (batchedParts.length > 0) {
			appStore.set(batchUpsertPartsAtom, batchedParts)
			// Bump per-session streaming version so the UI picks up the new parts
			for (const sid of batchedPartSessionIds) {
				appStore.set(streamingVersionFamily(sid), (v: number) => v + 1)
			}
		}
	}

	function enqueue(event: Event) {
		// Fast path: route high-frequency text/reasoning part updates to streaming buffer
		if (event.type === "message.part.updated") {
			const part = event.properties.part
			if (isStreamingPartType(part)) {
				updateStreamingPart(part)
				const key = coalescingKey(event)
				if (key) coalesced.set(key, event)
				if (scheduled !== undefined) return
				const elapsed = performance.now() - lastFlush
				if (elapsed < FRAME_BUDGET_MS) {
					scheduled = requestAnimationFrame(flush)
				} else {
					flush()
				}
				return
			}
		}

		// Fast path: route incremental text/reasoning deltas to streaming buffer
		if (event.type === "message.part.delta") {
			const { messageID, partID, field, delta, sessionID } = event.properties
			if (isStreamingField(field)) {
				const applied = applyStreamingDelta(messageID, partID, field, delta, sessionID)
				if (applied) {
					const key = coalescingKey(event)
					if (key) coalesced.set(key, event)
					if (scheduled !== undefined) return
					const elapsed = performance.now() - lastFlush
					if (elapsed < FRAME_BUDGET_MS) {
						scheduled = requestAnimationFrame(flush)
					} else {
						flush()
					}
					return
				}
				// Part not in streaming buffer yet, fall through to normal processing
			}
		}

		// When a session goes idle, flush streaming parts to main store
		if (event.type === "session.status" && event.properties.status.type === "idle") {
			const flushedParts = flushStreamingParts()
			if (flushedParts.length > 0) {
				appStore.set(batchUpsertPartsAtom, flushedParts)
			}
		}

		const key = coalescingKey(event)
		if (key) {
			coalesced.set(key, event)
		} else {
			queue.push(event)
		}

		if (scheduled !== undefined) return

		const elapsed = performance.now() - lastFlush
		if (elapsed < FRAME_BUDGET_MS) {
			scheduled = requestAnimationFrame(flush)
		} else {
			flush()
		}
	}

	function dispose(discard = false) {
		if (scheduled !== undefined) {
			cancelAnimationFrame(scheduled)
			scheduled = undefined
		}
		if (discard) {
			// Stale connection — drop buffered events instead of flushing them.
			// Flushing would re-add sessions to the store after sessionIdsAtom has
			// already been cleared by triggerServerSwitch(), causing stale sessions
			// from the previous server to reappear in the sidebar.
			queue = []
			coalesced.clear()
		} else {
			flush()
		}
	}

	return { enqueue, dispose }
}

// ============================================================
// Native Event Loop
// ============================================================

async function startEventLoop(
	client: DevoClient,
	signal: AbortSignal,
	generation: number,
): Promise<void> {
	let retryDelay = 1000

	const isStale = () => signal.aborted || generation !== eventLoopGeneration

	/** Only write serverConnectedAtom if this event loop is still the active one. */
	const setConnected = (value: boolean) => {
		if (!isStale()) appStore.set(serverConnectedAtom, value)
	}

	log.info("Native event loop started", { generation })

	while (!isStale()) {
		// Before opening the Native event stream, check if the server is reachable.
		// This avoids firing expensive IPC event requests against a dead server.
		// On the first iteration the caller already ran a health check, so
		// we only probe when retrying (retryDelay > 1000 means we already failed once).
		if (retryDelay > 1000 || !appStore.get(serverConnectedAtom)) {
			const healthy = await checkHealth(connection?.url ?? "", connection?.authHeader ?? null)
			if (isStale()) break
			setConnected(healthy)
			if (!healthy) {
				log.warn("Server health check failed, backing off", { generation, retryDelay })
				await new Promise((resolve) => setTimeout(resolve, retryDelay))
				retryDelay = Math.min(retryDelay * 2, 30000)
				continue
			}
		}

		const batcher = createEventBatcher()

		try {
			log.debug("Opening Native event stream", { generation })
			const stream = await subscribeToGlobalEvents(client)
			if (isStale()) break
			retryDelay = 1000
			log.info("Native event stream connected", { generation })

			// Native event stream opened successfully, server is reachable
			setConnected(true)

			for await (const globalEvent of stream) {
				if (isStale()) break
				const event = globalEvent.payload
				if (event) {
					batcher.enqueue(event)
				}
			}
			if (!isStale()) {
				log.warn("Native event stream ended (server closed connection)", { generation })
				setConnected(false)
			}
		} catch (err) {
			if (isStale()) break
			log.error("Native event stream disconnected", { generation, retryDelay }, err)
			setConnected(false)
		} finally {
			// Discard pending events when the loop is stale (server switched / disconnected).
			// Flushing stale events would re-populate session atoms that were just cleared.
			batcher.dispose(isStale())
		}

		if (isStale()) break

		log.info("Reconnecting Native event stream in", { delayMs: retryDelay, generation })
		await new Promise((resolve) => setTimeout(resolve, retryDelay))
		retryDelay = Math.min(retryDelay * 2, 30000)
	}

	log.info("Native event loop exited", { generation, stale: generation !== eventLoopGeneration })
}

function startProjectEventBridge(client: DevoClient, directory: string, signal: AbortSignal): void {
	if (projectEventBridgeDirs.has(directory)) return
	projectEventBridgeDirs.add(directory)

	void (async () => {
		const batcher = createEventBatcher()
		try {
			const stream = await subscribeToGlobalEvents(client)
			for await (const globalEvent of stream) {
				if (signal.aborted) break
				const event = globalEvent.payload
				if (event) {
					batcher.enqueue(event)
				}
			}
		} catch (err) {
			if (!signal.aborted) {
				log.warn("Project event bridge stopped", { directory }, err)
			}
		} finally {
			batcher.dispose(signal.aborted)
			projectEventBridgeDirs.delete(directory)
		}
	})()
}
