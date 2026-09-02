import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type { QueueWireEntry } from "../lib/queue-helpers"

export const sessionQueueFamily = atomFamily((_sessionId: string) => atom<QueueWireEntry[]>([]))

export const sessionActiveTurnFamily = atomFamily((_sessionId: string) =>
	atom<string | null>(null),
)

export const setSessionQueueAtom = atom(
	null,
	(_get, set, params: { sessionId: string; entries: QueueWireEntry[] }) => {
		set(sessionQueueFamily(params.sessionId), params.entries)
	},
)

export const setSessionActiveTurnAtom = atom(
	null,
	(_get, set, params: { sessionId: string; turnId: string | null }) => {
		set(sessionActiveTurnFamily(params.sessionId), params.turnId)
	},
)
