import { createDevoClient, type DevoNativeTransport } from "@devo-ai/sdk/v2/client"
import { ProtocolValidationError } from "@devo-ai/sdk/v2/protocol-validation"
import { createLogger } from "./logger"
import {
	applyWatcherEvent,
	type GlobalNativeEvent,
	type SessionState as WatcherSessionState,
} from "./notification-policy"
import { setPermissionResponder, showNotification, updateBadgeCount } from "./notifications"
import { recycleServerForProtocolMismatch } from "./devo-manager"

const log = createLogger("notification-watcher")

export type SessionState = WatcherSessionState

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

/**
 * True while draining subscription snapshot/replay. Those events look like
 * live busy→idle completions and would toast every idle session on app open.
 */
let hydrating = false
let hydrationEndTimer: ReturnType<typeof setImmediate> | null = null

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
	beginHydration()
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
			if (isProtocolValidationError(err)) {
				// Our stdio child may be proxying to a stale singleton server
				// whose wire shape predates this build's schema. Recycling shuts
				// the singleton down and restarts our own server; restartServer()
				// stops this watcher and starts a fresh one, so exit the loop.
				log.error(
					"Native protocol validation failed",
					{ reason: describeProtocolMismatch(err) },
					describeOffendingPayloadEntry(err),
				)
				const recycled = await recycleServerForProtocolMismatch(describeProtocolMismatch(err))
				if (recycled) return
			}
			log.error("Native event stream error, reconnecting", { retryDelay }, err)
		}

		if (signal.aborted) break

		// Exponential backoff: 1s -> 2s -> 4s -> ... -> 30s max
		await sleep(retryDelay, signal)
		retryDelay = Math.min(retryDelay * 2, 30_000)
	}
}

function isProtocolValidationError(error: unknown): error is ProtocolValidationError {
	if (error instanceof ProtocolValidationError) return true
	return (
		typeof error === "object" &&
		error !== null &&
		(error as { name?: unknown }).name === "ProtocolValidationError"
	)
}

function describeProtocolMismatch(error: ProtocolValidationError): string {
	return `${error.method} (${error.schemaName ?? error.direction})`
}

/**
 * Extracts the replay entry an ajv failure pointed at (instancePath like
 * `/replay/5/notification/method`) so the next occurrence is diagnosable from
 * the log alone — the Electron log formatter renders nested objects as
 * `[Object]`, hiding the offending method otherwise.
 */
function describeOffendingPayloadEntry(error: ProtocolValidationError): string {
	const match = error.errors
		.map((ajvError) => /^\/replay\/(\d+)\//.exec(ajvError.instancePath ?? ""))
		.find(Boolean)
	if (!match) return ""
	const entry = (error.payload as { replay?: unknown[] } | null)?.replay?.[Number(match[1])]
	if (entry === undefined) return ""
	try {
		return JSON.stringify(entry).slice(0, 800)
	} catch {
		return String(entry).slice(0, 800)
	}
}

async function consumeNativeEvents(client: ReturnType<typeof createDevoClient>, signal: AbortSignal): Promise<void> {
	beginHydration()
	const result = await client.event.subscribe()
	if (signal.aborted) return
	log.info("Native event stream connected")
	// Replay is already queued when subscribe() returns. Drain it as microtasks
	// with hydrating=true, then enable live toasts on the next event-loop turn.
	scheduleHydrationEnd()
	try {
		for await (const globalEvent of result.stream) {
			if (signal.aborted) break
			processGlobalEvent(globalEvent)
		}
	} finally {
		beginHydration()
	}
}

function beginHydration(): void {
	hydrating = true
	if (hydrationEndTimer !== null) {
		clearImmediate(hydrationEndTimer)
		hydrationEndTimer = null
	}
}

function scheduleHydrationEnd(): void {
	if (hydrationEndTimer !== null) clearImmediate(hydrationEndTimer)
	hydrationEndTimer = setImmediate(() => {
		hydrationEndTimer = null
		hydrating = false
		log.debug("Notification watcher hydration complete")
	})
}

// ============================================================
// Event Processing — only notification-relevant events
// ============================================================

function processGlobalEvent(globalEvent: GlobalNativeEvent): void {
	const result = applyWatcherEvent(globalEvent, {
		sessions,
		pendingCount,
		hydrating,
	})
	if (!result.stateChanged && result.notifications.length === 0) return

	pendingCount = result.pendingCount
	if (result.stateChanged) {
		updateBadgeCount(pendingCount)
		scheduleNotify()
	}
	for (const notification of result.notifications) {
		showNotification(notification)
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
