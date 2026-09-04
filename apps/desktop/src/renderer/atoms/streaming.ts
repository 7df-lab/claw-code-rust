import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type { Part } from "../lib/types"
import { partStorageKey } from "./parts"
import { appStore } from "./store"

// ============================================================
// Streaming Buffer — module-scoped (NOT atoms)
// Same pattern as the old streaming-store.ts but notifies
// via per-session Jotai version counters so only the affected
// session's components re-render.
// ============================================================

/** Throttle interval for React notifications — ~12.5 updates/sec */
const FLUSH_THROTTLE_MS = 80

/** Parts keyed by sessionID + messageID -> Part object, only for actively-streaming parts */
let buffer: Record<string, Record<string, Part>> = {}

// ============================================================
// Per-session notification
// ============================================================

/**
 * Per-session version counter. Only the session that is actively streaming
 * will have its counter bumped, so components for other sessions stay idle.
 */
export const streamingVersionFamily = atomFamily((_sessionId: string) => atom(0))

/**
 * @deprecated Use `streamingVersionFamily(sessionId)` instead.
 * Kept temporarily so any transient consumers still compile.
 */
export const streamingVersionAtom = atom(0)

/** Per-session throttle state */
const sessionThrottle = new Map<
	string,
	{ scheduled: ReturnType<typeof setTimeout> | undefined; lastFlush: number }
>()

/** Set of session IDs that have been dirtied since the last per-session flush. */
const dirtySessionIds = new Set<string>()

function getThrottle(sessionId: string) {
	let t = sessionThrottle.get(sessionId)
	if (!t) {
		t = { scheduled: undefined, lastFlush: 0 }
		sessionThrottle.set(sessionId, t)
	}
	return t
}

function notifySession(sessionId: string): void {
	const t = getThrottle(sessionId)
	t.scheduled = undefined
	t.lastFlush = performance.now()
	dirtySessionIds.delete(sessionId)
	appStore.set(streamingVersionFamily(sessionId), (v) => v + 1)
}

function scheduleNotifySession(sessionId: string): void {
	const t = getThrottle(sessionId)
	if (t.scheduled) return
	const elapsed = performance.now() - t.lastFlush
	if (elapsed >= FLUSH_THROTTLE_MS) {
		notifySession(sessionId)
	} else {
		t.scheduled = setTimeout(() => notifySession(sessionId), FLUSH_THROTTLE_MS - elapsed)
	}
}

// ============================================================
// Session ID lookup — messageID -> sessionID
// ============================================================

/**
 * Lightweight map from scoped message key to sessionID. Populated when streaming
 * parts arrive (Part.sessionID is on every SDK Part variant).
 */
const messageSessionMap = new Map<string, string>()
const bufferMessageIdMap = new Map<string, string>()

/** Look up which session a message belongs to. */
export function getSessionForMessage(messageId: string): string | undefined {
	return messageSessionMap.get(messageId)
}

/**
 * Get streaming parts scoped to a specific session.
 * Only returns parts belonging to messages in that session.
 */
export function getStreamingPartsForSession(
	sessionId: string,
): Record<string, Record<string, Part>> {
	const result: Record<string, Record<string, Part>> = {}
	for (const key in buffer) {
		if (messageSessionMap.get(key) === sessionId) {
			const messageId = bufferMessageIdMap.get(key)
			if (messageId) {
				result[messageId] = buffer[key]
			}
		}
	}
	return result
}

// ============================================================
// Public API — called from the event batcher (non-React code)
// ============================================================

/**
 * Write a part to the streaming buffer.
 * Called by connection-manager on every text/reasoning Native event.
 */
export function updateStreamingPart(part: Part): void {
	const { messageID, sessionID } = part
	// Register the messageID -> sessionID mapping
	const key = partStorageKey(sessionID, messageID)
	messageSessionMap.set(key, sessionID)
	bufferMessageIdMap.set(key, messageID)

	if (!buffer[key]) buffer[key] = {}
	buffer[key][part.id] = part

	dirtySessionIds.add(sessionID)
	scheduleNotifySession(sessionID)
}

/**
 * Apply a string delta to a field of an existing part in the streaming buffer.
 * Used for incremental text/reasoning updates (message.part.delta events).
 * Returns true if the delta was applied, false if the part was not found.
 */
export function applyStreamingDelta(
	messageId: string,
	partId: string,
	field: string,
	delta: string,
	sessionId: string,
): boolean {
	const key = partStorageKey(sessionId, messageId)
	messageSessionMap.set(key, sessionId)
	bufferMessageIdMap.set(key, messageId)

	const msgParts = buffer[key]
	if (!msgParts?.[partId]) return false

	const part = msgParts[partId]
	// Part is a discriminated union; the server sends the field name as a plain string.
	const record = part as Record<string, unknown>
	const existing = record[field]
	// Create a shallow copy with the appended field value
	msgParts[partId] = { ...part, [field]: (typeof existing === "string" ? existing : "") + delta }

	dirtySessionIds.add(sessionId)
	scheduleNotifySession(sessionId)
	return true
}

/**
 * Check if a part type should go through streaming buffer.
 */
export function isStreamingPartType(part: Part): boolean {
	return part.type === "text" || part.type === "reasoning"
}

/**
 * Check if a field name corresponds to a streamable text field.
 */
export function isStreamingField(field: string): boolean {
	return field === "content" || field === "text" || field === "reasoning"
}

/**
 * Get streaming parts for a specific message (used in derived atoms).
 */
export function getStreamingPartsForMessage(messageId: string): Record<string, Part> | undefined {
	return buffer[messageId]
}

/**
 * Get all streaming parts.
 */
export function getAllStreamingParts(): Record<string, Record<string, Part>> {
	return buffer
}

/**
 * Check if there are any buffered streaming parts.
 */
export function hasStreamingParts(): boolean {
	return Object.keys(buffer).length > 0
}

/**
 * Get the current streaming part for a given session + message + part ID.
 */
export function getStreamingPart(
	sessionId: string,
	messageId: string,
	partId: string,
): Part | undefined {
	return buffer[partStorageKey(sessionId, messageId)]?.[partId]
}

/**
 * Flush all streaming parts into the main Jotai atoms.
 * Called when a session goes idle.
 * Returns the flushed parts as a flat array for batch upsert.
 */
export function flushStreamingParts(): Part[] {
	// Cancel all pending per-session timers
	for (const [, t] of sessionThrottle) {
		if (t.scheduled) {
			clearTimeout(t.scheduled)
			t.scheduled = undefined
		}
	}

	const allParts: Part[] = []
	const affectedSessions = new Set<string>()

	for (const key in buffer) {
		const sid = messageSessionMap.get(key)
		if (sid) affectedSessions.add(sid)
		for (const partId in buffer[key]) {
			allParts.push(buffer[key][partId])
		}
	}

	buffer = {}
	bufferMessageIdMap.clear()
	dirtySessionIds.clear()

	// Notify affected sessions that streaming data has been flushed
	for (const sid of affectedSessions) {
		appStore.set(streamingVersionFamily(sid), (v) => v + 1)
	}

	return allParts
}
