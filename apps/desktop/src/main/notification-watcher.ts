import { createDevoClient, type DevoNativeTransport } from "@devo-ai/sdk/v2/client"
import { createLogger } from "./logger"
import { setPermissionResponder, showNotification, updateBadgeCount } from "./notifications"

const log = createLogger("notification-watcher")

// ============================================================
// Types — minimal, only what we need for notification decisions
// ============================================================

export interface SessionState {
	status: string // "busy" | "idle" | "retry"
	title: string
	directory?: string
	/** If set, this session is a sub-agent spawned by another session. */
	parentID?: string
}

// ============================================================
// State
// ============================================================

let abortController: AbortController | null = null

/** Minimal session state for transition detection. */
const sessions = new Map<string, SessionState>()

/** Pending permission/question count for badge. */
let pendingCount = 0

/** Listeners notified whenever session or pending state changes. */
const changeListeners = new Set<() => void>()

// ============================================================
// Public API
// ============================================================

/**
 * Start watching the Devo server's Native event stream
 * for notification-worthy events.
 *
 * This runs in the main process (Node.js) and is never throttled
 * by Chromium's background tab restrictions or macOS App Nap.
 */
export function startNotificationWatcher(transport: DevoNativeTransport): void {
	if (abortController) {
		log.debug("Stopping existing watcher before restart")
		abortController.abort()
	}

	abortController = new AbortController()
	pendingCount = 0

	const client = createDevoClient({ transport })
	setPermissionResponder(async ({ sessionId, permissionId, response }) => {
		await client.permission.respond({
			sessionID: sessionId,
			permissionID: permissionId,
			response,
		})
	})

	log.info("Starting notification watcher")
	connectWithRetry(client, abortController.signal)
}

/**
 * Stop the notification watcher.
 */
export function stopNotificationWatcher(): void {
	if (abortController) {
		abortController.abort()
		abortController = null
	}
	sessions.clear()
	pendingCount = 0
	updateBadgeCount(0)
	setPermissionResponder(null)
	log.info("Notification watcher stopped")
}

/**
 * Check if the watcher is currently running.
 */
export function isWatcherRunning(): boolean {
	return abortController !== null && !abortController.signal.aborted
}

/**
 * Get a snapshot of all tracked sessions.
 * Returns a new Map (caller-safe to iterate without races).
 */
export function getSessionStates(): ReadonlyMap<string, SessionState> {
	return new Map(sessions)
}

/**
 * Get the current pending permission/question count.
 */
export function getPendingCount(): number {
	return pendingCount
}

/**
 * Subscribe to any state change (session status, pending count).
 * Called after every processGlobalEvent that mutates state.
 * Returns an unsubscribe function.
 */
export function onStateChanged(listener: () => void): () => void {
	changeListeners.add(listener)
	return () => changeListeners.delete(listener)
}

// ============================================================
// Native Connection + Retry Loop
// ============================================================

async function connectWithRetry(client: ReturnType<typeof createDevoClient>, signal: AbortSignal): Promise<void> {
	let retryDelay = 1_000

	while (!signal.aborted) {
		try {
			await consumeNativeEvents(client, signal)
			// Stream ended normally (server closed connection)
			if (!signal.aborted) {
				log.warn("Native event stream ended, reconnecting...")
			}
		} catch (err) {
			if (signal.aborted) break
			log.error("Native event stream error, reconnecting", { retryDelay }, err)
		}

		if (signal.aborted) break

		// Exponential backoff: 1s -> 2s -> 4s -> ... -> 30s max
		await sleep(retryDelay, signal)
		retryDelay = Math.min(retryDelay * 2, 30_000)
	}
}

async function consumeNativeEvents(client: ReturnType<typeof createDevoClient>, signal: AbortSignal): Promise<void> {
	const result = await client.event.subscribe()
	log.info("Native event stream connected")
	for await (const globalEvent of result.stream) {
		if (signal.aborted) break
		processGlobalEvent(globalEvent)
	}
}

// ============================================================
// Event Processing — only notification-relevant events
// ============================================================

interface GlobalNativeEvent {
	directory?: string
	payload?: {
		type: string
		properties: Record<string, unknown>
	}
}

function processGlobalEvent(globalEvent: GlobalNativeEvent): void {
	const event = globalEvent.payload
	if (!event) return

	const eventType = event.type
	const props = event.properties

	const directory = globalEvent.directory

	switch (eventType) {
		case "permission.asked": {
			const sessionId = props.sessionID as string
			const permission = (props as { permission?: string }).permission
			// Always count sub-agent permissions — they block the parent too.
			// Attribute the notification to the root session so clicking it
			// navigates to the parent where the user can respond.
			const rootId = getRootSession(sessionId)
			const notifySessionId = rootId
			const rootTitle = sessions.get(rootId)?.title
			const rootDir = sessions.get(rootId)?.directory ?? directory
			pendingCount++
			updateBadgeCount(pendingCount)
			showNotification({
				type: "permission",
				sessionId: notifySessionId,
				title: isSubAgent(sessionId)
					? `Sub-agent needs permission${rootTitle ? ` — ${rootTitle}` : ""}`
					: "Agent needs permission",
				body: permission || "Approval required",
				directory: rootDir,
				meta: { permissionId: props.id as string },
			})
			scheduleNotify()
			break
		}

		case "permission.replied": {
			pendingCount = Math.max(0, pendingCount - 1)
			updateBadgeCount(pendingCount)
			scheduleNotify()
			break
		}

		case "question.asked": {
			const sessionId = props.sessionID as string
			const questions = props.questions as Array<{ header?: string }> | undefined
			const header = questions?.[0]?.header ?? "Question"
			// Same bubbling logic as permission.asked.
			const rootId = getRootSession(sessionId)
			const rootTitle = sessions.get(rootId)?.title
			const rootDir = sessions.get(rootId)?.directory ?? directory
			pendingCount++
			updateBadgeCount(pendingCount)
			showNotification({
				type: "question",
				sessionId: rootId,
				title: isSubAgent(sessionId)
					? `Sub-agent has a question${rootTitle ? ` — ${rootTitle}` : ""}`
					: "Agent has a question",
				body: header,
				directory: rootDir,
				meta: { requestId: props.id as string },
			})
			scheduleNotify()
			break
		}

		case "question.replied":
		case "question.rejected": {
			pendingCount = Math.max(0, pendingCount - 1)
			updateBadgeCount(pendingCount)
			scheduleNotify()
			break
		}

		case "session.status": {
			const sessionId = props.sessionID as string
			const newStatusType = (props.status as { type: string })?.type
			if (!sessionId || !newStatusType) break

			const prev = sessions.get(sessionId)
			const prevStatus = prev?.status

			// Update tracked state
			sessions.set(sessionId, {
				status: newStatusType,
				title: prev?.title ?? "",
				directory: directory ?? prev?.directory,
				parentID: prev?.parentID,
			})
			scheduleNotify()

			// Detect busy/retry -> idle transition (agent completed)
			if (
				newStatusType === "idle" &&
				(prevStatus === "busy" || prevStatus === "retry") &&
				!isSubAgent(sessionId)
			) {
				const sessionTitle = sessions.get(sessionId)?.title
				showNotification({
					type: "completed",
					sessionId,
					title: "Agent finished",
					body: sessionTitle || "Task completed",
					directory,
				})
			}
			break
		}

		case "session.error": {
			const sessionId = props.sessionID as string
			const error = props.error as { name?: string } | undefined
			if (!sessionId) break
			if (!isSubAgent(sessionId)) {
				showNotification({
					type: "error",
					sessionId,
					title: "Agent encountered an error",
					body: error?.name ?? "Unknown error",
					directory,
				})
			}
			break
		}

		case "session.created":
		case "session.updated": {
			// Track session title, directory, and parentID for use in notification decisions
			const info = (props.info ?? props.session) as
				| { id?: string; title?: string; parentID?: string }
				| undefined
			if (info?.id) {
				const existing = sessions.get(info.id)
				sessions.set(info.id, {
					status: existing?.status ?? "idle",
					title: info.title ?? existing?.title ?? "",
					directory: directory ?? existing?.directory,
					parentID: info.parentID ?? existing?.parentID,
				})
				scheduleNotify()
			}
			break
		}

		// All other events (message.*, todo.*, etc.) are ignored —
		// they're the renderer's domain.
	}
}

// ============================================================
// Helpers
// ============================================================

/** Notify all change listeners (debounced per event loop tick). */
let notifyScheduled = false
function scheduleNotify(): void {
	if (notifyScheduled) return
	notifyScheduled = true
	queueMicrotask(() => {
		notifyScheduled = false
		for (const listener of changeListeners) {
			try {
				listener()
			} catch {
				// Listener errors must not break the watcher
			}
		}
	})
}

/** Check if a session is a sub-agent (has a parent session). */
function isSubAgent(sessionId: string): boolean {
	return !!sessions.get(sessionId)?.parentID
}

/**
 * Walk up the parentID chain to find the top-level (root) session for a given
 * session ID.  Returns the session ID itself when there is no parent.
 * Guards against cycles with a depth limit.
 */
function getRootSession(sessionId: string): string {
	let id = sessionId
	const seen = new Set<string>()
	while (true) {
		if (seen.has(id)) break // cycle guard
		seen.add(id)
		const parentID = sessions.get(id)?.parentID
		if (!parentID) break
		id = parentID
	}
	return id
}

function sleep(ms: number, signal: AbortSignal): Promise<void> {
	return new Promise((resolve) => {
		if (signal.aborted) {
			resolve()
			return
		}
		const timer = setTimeout(resolve, ms)
		signal.addEventListener(
			"abort",
			() => {
				clearTimeout(timer)
				resolve()
			},
			{ once: true },
		)
	})
}
