import { useCallback, useEffect, useState } from "react"
import {
	evictMountedSession,
	initialMountedSessionIds,
	updateMountedSessionIds,
} from "./use-session-keep-alive-logic"

export {
	SESSION_KEEP_ALIVE_CAPACITY,
	evictMountedSession,
	initialMountedSessionIds,
	updateMountedSessionIds,
} from "./use-session-keep-alive-logic"

export function useSessionKeepAlive(activeSessionId: string | null) {
	const [mountedSessionIds, setMountedSessionIds] = useState<string[]>(() =>
		initialMountedSessionIds(activeSessionId),
	)

	const evictSession = useCallback((sessionId: string) => {
		setMountedSessionIds((prev) => evictMountedSession(prev, sessionId))
	}, [])

	useEffect(() => {
		if (!activeSessionId) return
		setMountedSessionIds((prev) => updateMountedSessionIds(prev, activeSessionId))
	}, [activeSessionId])

	return { mountedSessionIds, evictSession }
}
