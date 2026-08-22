import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useEffect, useRef, useState } from "react"
import { isMockModeAtom } from "../atoms/mock-mode"
import {
	diffFilterFamily,
	sessionDiffFamily,
	sessionDiffStatsFamily,
	setSessionDiffAtom,
} from "../atoms/ui"
import type { FileDiff } from "../lib/types"
import { getProjectClient } from "../services/connection-manager"
import { getSessionDiff } from "../services/devo"

/**
 * Hook that fetches session diffs and subscribes to real-time updates
 * via the Native event processor (which writes to sessionDiffFamily).
 *
 * Returns the current diffs, loading state, aggregate stats, and a refetch function.
 */
export function useSessionDiff(sessionId: string, directory: string) {
	const diffs = useAtomValue(sessionDiffFamily(sessionId))
	const stats = useAtomValue(sessionDiffStatsFamily(sessionId))
	const setDiffs = useSetAtom(setSessionDiffAtom)
	const filter = useAtomValue(diffFilterFamily(sessionId))
	const isMockMode = useAtomValue(isMockModeAtom)
	const loadingRef = useRef(false)
	const fetchedRef = useRef<string | null>(null)
	const [initialFetchDone, setInitialFetchDone] = useState(false)

	const fetchDiffs = useCallback(async () => {
		if (isMockMode) return
		if (loadingRef.current) return
		loadingRef.current = true
		try {
			const client = getProjectClient(directory)
			if (!client) return
			const result = await getSessionDiff(client, sessionId)
			setDiffs({ sessionId, diffs: result })
		} catch {
			// Silently fail, diffs will update via Native events
		} finally {
			loadingRef.current = false
			setInitialFetchDone(true)
		}
	}, [sessionId, directory, setDiffs, isMockMode])

	// Initial fetch when session changes
	useEffect(() => {
		if (isMockMode) return
		if (fetchedRef.current !== sessionId) {
			fetchedRef.current = sessionId
			setInitialFetchDone(false)
			fetchDiffs()
		}
	}, [sessionId, fetchDiffs, isMockMode])

	// Filter diffs by message if a filter is active
	const filteredDiffs: FileDiff[] = filter
		? diffs // TODO: implement per-message filtering when the API is wired
		: diffs

	return {
		diffs: filteredDiffs,
		allDiffs: diffs,
		stats,
		loading: !initialFetchDone,
		refetch: fetchDiffs,
		filter,
	}
}
