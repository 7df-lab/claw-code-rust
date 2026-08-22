import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type { Message, Part } from "../lib/types"
import { partsFamily, partStorageKey } from "./parts"

// ============================================================
// Helpers
// ============================================================

const MAX_MESSAGES_PER_SESSION = 200

function messageCreatedAt(message: Message): number {
	const created = message.time?.created
	return typeof created === "number" && Number.isFinite(created) ? created : 0
}

function compareMessages(left: Message, right: Message): number {
	const byCreated = messageCreatedAt(left) - messageCreatedAt(right)
	return byCreated === 0 ? left.id.localeCompare(right.id) : byCreated
}

function sortedMessages(messages: Iterable<Message>): Message[] {
	return [...messages].sort(compareMessages)
}

// ============================================================
// Per-session message list (sorted by creation time)
// ============================================================

export const messagesFamily = atomFamily((_sessionId: string) => atom<Message[]>([]))

// ============================================================
// Action atoms
// ============================================================

/**
 * Set messages for a session (initial fetch + merge with existing Native event data).
 */
export const setMessagesAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			messages: Message[]
			parts: Record<string, Part[]>
		},
	) => {
		const existing = get(messagesFamily(args.sessionId))

		// Fast path: no existing messages — just set everything
		if (!existing || existing.length === 0) {
			set(messagesFamily(args.sessionId), sortedMessages(args.messages))
			for (const [messageId, msgParts] of Object.entries(args.parts)) {
				set(partsFamily(partStorageKey(args.sessionId, messageId)), msgParts)
				set(partsFamily(messageId), msgParts)
			}
			return
		}

		// Merge: fetched data fills gaps, while Native event versions win for matching IDs.
		const byId = new Map(args.messages.map((message) => [message.id, message]))
		for (const message of existing) {
			byId.set(message.id, message)
		}
		const merged = sortedMessages(byId.values())

		// Merge parts: fetched parts fill in gaps, Native event parts take priority
		for (const [messageId, fetchedParts] of Object.entries(args.parts)) {
			const scopedKey = partStorageKey(args.sessionId, messageId)
			const existingScopedParts = get(partsFamily(scopedKey))
			if (!existingScopedParts || existingScopedParts.length === 0) {
				// No Native event parts yet for this message — use fetched
				set(partsFamily(scopedKey), fetchedParts)
			}
			const existingLegacyParts = get(partsFamily(messageId))
			if (!existingLegacyParts || existingLegacyParts.length === 0) {
				set(partsFamily(messageId), fetchedParts)
			}
			// Otherwise keep the Native-event-accumulated parts (more recent)
		}

		set(messagesFamily(args.sessionId), merged)
	},
)

/**
 * Upsert a single message.
 */
export const upsertMessageAtom = atom(null, (get, set, message: Message) => {
	const sessionId = message.sessionID
	let existing = get(messagesFamily(sessionId))

	// When a real user message arrives, remove the oldest optimistic placeholder.
	if (message.role === "user" && !message.id.startsWith("optimistic-")) {
		const optimisticIndex = existing.findIndex(
			(m) => m.id.startsWith("optimistic-") && m.role === "user",
		)
		if (optimisticIndex !== -1) {
			const optimisticId = existing[optimisticIndex].id
			// Clean up parts for the optimistic message
			set(partsFamily(partStorageKey(sessionId, optimisticId)), [])
			set(partsFamily(optimisticId), [])
			existing = existing.filter((_, i) => i !== optimisticIndex)
		}
	}

	const existingIndex = existing.findIndex((item) => item.id === message.id)
	if (existingIndex >= 0 && existing[existingIndex] === message) return

	const updated =
		existingIndex >= 0
			? existing.map((item, index) => (index === existingIndex ? message : item))
			: [...existing, message]
	updated.sort(compareMessages)

	// Cap at MAX_MESSAGES_PER_SESSION
	if (updated.length > MAX_MESSAGES_PER_SESSION) {
		const removed = updated.shift()!
		set(partsFamily(partStorageKey(sessionId, removed.id)), [])
		set(partsFamily(removed.id), [])
	}
	set(messagesFamily(sessionId), updated)
})

/**
 * Remove a message from a session.
 */
export const removeMessageAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			messageId: string
		},
	) => {
		const existing = get(messagesFamily(args.sessionId))
		if (!existing) return
		const existingIndex = existing.findIndex((message) => message.id === args.messageId)
		if (existingIndex < 0) return
	const updated = [...existing]
	updated.splice(existingIndex, 1)
	set(partsFamily(partStorageKey(args.sessionId, args.messageId)), [])
	set(partsFamily(args.messageId), [])
	set(messagesFamily(args.sessionId), updated)
	},
)
