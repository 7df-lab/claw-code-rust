export const SESSION_KEEP_ALIVE_CAPACITY = 3

export function updateMountedSessionIds(
	mountedSessionIds: string[],
	activeSessionId: string | null,
	capacity = SESSION_KEEP_ALIVE_CAPACITY,
): string[] {
	if (!activeSessionId) return mountedSessionIds
	const withoutActive = mountedSessionIds.filter((id) => id !== activeSessionId)
	return [activeSessionId, ...withoutActive].slice(0, capacity)
}

export function evictMountedSession(mountedSessionIds: string[], sessionId: string): string[] {
	return mountedSessionIds.filter((id) => id !== sessionId)
}

export function initialMountedSessionIds(activeSessionId: string | null): string[] {
	return activeSessionId ? [activeSessionId] : []
}
