import { useAtomValue } from "jotai"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
	type ChatMessageEntry,
	type ChatTurn,
	groupIntoTurns,
	mergeSessionParts,
} from "../atoms/derived/session-chat"
import { messagesFamily, setMessagesAtom } from "../atoms/messages"
import { isMockModeAtom } from "../atoms/mock-mode"
import { partsFamily, partStorageKey } from "../atoms/parts"
import { appStore } from "../atoms/store"
import { streamingVersionFamily } from "../atoms/streaming"
import { queryClient } from "../lib/query-client"
import type { Message, Part } from "../lib/types"
import { getBaseClient, getProjectClient } from "../services/connection-manager"
import { queryKeys } from "./use-devo-data"

// Re-export types for consumers
export type { ChatMessageEntry, ChatTurn }

/** Sentinel empty array — stable reference */
const EMPTY_ENTRIES: ChatMessageEntry[] = []

/**
 * History is paginated by message count (aligned to user-message boundaries).
 * Agent turns with many tool calls can span dozens of messages, so budget
 * generously when targeting a turn count.
 */
const MESSAGES_PER_TURN_ESTIMATE = 20
/** Turns shown when opening a historical session. */
const INITIAL_TURN_COUNT = 8
/** Additional turns fetched each time the user scrolls toward the top. */
const PAGE_TURN_COUNT = 5

const INITIAL_LIMIT = INITIAL_TURN_COUNT * MESSAGES_PER_TURN_ESTIMATE
const PAGE_SIZE = PAGE_TURN_COUNT * MESSAGES_PER_TURN_ESTIMATE

/**
 * Hook to load chat data for a session.
 *
 * - Reads messages/parts from Jotai atoms (populated by Native events)
 * - Does a one-time initial fetch to hydrate the store
 * - Uses structural sharing in `groupIntoTurns` to preserve React.memo()
 * - No polling — Native events keep data up to date
 * - Subscribes to the per-session streaming version so only updates for
 *   THIS session trigger re-renders (not all sessions globally).
 */
export function useSessionChat(
	directory: string | null,
	sessionId: string | null,
	_isActive = false,
) {
	const isMockMode = useAtomValue(isMockModeAtom)
	const [loading, setLoading] = useState(false)
	const [loadingEarlier, setLoadingEarlier] = useState(false)
	const [hasEarlierMessages, setHasEarlierMessages] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const syncedRef = useRef<string | null>(null)
	const turnsRef = useRef<ChatTurn[]>([])
	const loadedLimitsRef = useRef(new Map<string, number>())

	// Read from Jotai atoms
	const storeMessages = useAtomValue(messagesFamily(sessionId ?? ""))
	// Per-session streaming version: only bumped when THIS session streams
	const streamingVersion = useAtomValue(streamingVersionFamily(sessionId ?? ""))

	// Build ChatMessageEntry[] merging streaming overlay
	const entries: ChatMessageEntry[] = useMemo(() => {
		if (!storeMessages || storeMessages.length === 0) return EMPTY_ENTRIES
		return mergeSessionParts(
			sessionId ?? "",
			storeMessages,
			(messageId) => appStore.get(partsFamily(partStorageKey(sessionId ?? "", messageId))),
			streamingVersion,
		)
	}, [storeMessages, streamingVersion, sessionId])

	// Group into turns with structural sharing
	const turns = useMemo(() => {
		const result = groupIntoTurns(entries, turnsRef.current)
		turnsRef.current = result
		return result
	}, [entries])

	// One-time fetch to hydrate the store when session changes
	const fetchAndHydrate = useCallback(
		async (sid: string) => {
			// Only show the loading spinner if the session has no cached data yet.
			// When switching back to a previously-visited session the existing messages
			// remain visible while the background refresh runs, avoiding a jarring flash.
			const hasCachedData = (appStore.get(messagesFamily(sid)) ?? []).length > 0
			if (!hasCachedData) {
				setLoading(true)
			}
			setError(null)
			try {
				// Use a directory-scoped client when available, otherwise fall back to the base client
				const client = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
				if (!client) {
					setError("Not connected to Devo server")
					return
				}

				const limit = loadedLimitsRef.current.get(sid) ?? INITIAL_LIMIT
				const result = await client.session.messages({
					sessionID: sid,
					limit,
				})
				const raw = (result.data ?? []) as Array<{ info: Message; parts: Part[] }>
				loadedLimitsRef.current.set(sid, limit)
				setHasEarlierMessages(raw.length >= limit)

				// Hydrate the Jotai store
				const messages = raw.map((m) => m.info)
				const parts: Record<string, Part[]> = {}
				for (const m of raw) {
					parts[m.info.id] = m.parts
				}
				appStore.set(setMessagesAtom, { sessionId: sid, messages, parts })
				if (directory) {
					queryClient.invalidateQueries({ queryKey: queryKeys.providers(directory) })
					queryClient.invalidateQueries({ queryKey: queryKeys.config(directory) })
				}
			} catch (err) {
				console.error("Failed to fetch session messages:", err)
				setError(err instanceof Error ? err.message : "Failed to load messages")
			} finally {
				setLoading(false)
			}
		},
		[directory],
	)

	// Load the next page of older messages when the user scrolls toward the top.
	const loadEarlier = useCallback(async () => {
		if (!sessionId || !directory || loadingEarlier || !hasEarlierMessages) return
		const client = getProjectClient(directory)
		if (!client) return

		const currentLimit = loadedLimitsRef.current.get(sessionId) ?? INITIAL_LIMIT
		const nextLimit = currentLimit + PAGE_SIZE

		setLoadingEarlier(true)
		try {
			const result = await client.session.messages({
				sessionID: sessionId,
				limit: nextLimit,
			})
			const raw = (result.data ?? []) as Array<{ info: Message; parts: Part[] }>
			loadedLimitsRef.current.set(sessionId, nextLimit)
			setHasEarlierMessages(raw.length >= nextLimit)

			const messages = raw.map((m) => m.info)
			const parts: Record<string, Part[]> = {}
			for (const m of raw) {
				parts[m.info.id] = m.parts
			}
			appStore.set(setMessagesAtom, { sessionId, messages, parts })
			queryClient.invalidateQueries({ queryKey: queryKeys.providers(directory) })
			queryClient.invalidateQueries({ queryKey: queryKeys.config(directory) })
		} catch (err) {
			console.error("Failed to load earlier messages:", err)
		} finally {
			setLoadingEarlier(false)
		}
	}, [sessionId, directory, loadingEarlier, hasEarlierMessages])

	// Trigger initial fetch when session changes (skip in mock mode -- data is pre-hydrated)
	useEffect(() => {
		if (isMockMode) return
		if (!sessionId) return
		if (syncedRef.current === sessionId) return
		syncedRef.current = sessionId
		fetchAndHydrate(sessionId)
	}, [sessionId, fetchAndHydrate, isMockMode])

	// Reset per-session refs whenever the active session changes.
	//
	// - turnsRef: structural-sharing cache — must be cleared so stale turn objects
	//   from the previous session aren't mixed into the new session's render.
	// - hasEarlierMessages: tracks whether the server has older messages to load.
	//   MUST be cleared so the load-earlier affordance from a previous session
	//   doesn't appear on a freshly-switched session whose atom is still empty.
	// - loading: if the new session has no cached data yet, pre-set the loading
	//   flag so the UI shows a spinner instead of "No messages yet" during the
	//   one render that happens before the fetch effect fires.
	useEffect(() => {
		turnsRef.current = []
		setHasEarlierMessages(false)
		if (!isMockMode && sessionId) {
			const hasCachedData = (appStore.get(messagesFamily(sessionId)) ?? []).length > 0
			if (!hasCachedData) {
				setLoading(true)
			}
		}
	}, [sessionId, isMockMode])

	return {
		turns,
		rawMessages: entries,
		loading,
		loadingEarlier,
		error,
		hasEarlierMessages,
		loadEarlier,
		reload: fetchAndHydrate,
	}
}
