import { type MutableRefObject, useEffect } from "react"
import { cn } from "@devo/ui/lib/utils"
import { useSessionKeepAlive } from "../hooks/use-session-keep-alive"
import { SessionView } from "./session-view"

interface SessionKeepAliveHostProps {
	activeSessionId: string | null
	/** Optional ref populated with evictSession for sidebar delete handling. */
	evictRef?: MutableRefObject<(sessionId: string) => void>
}

export function SessionKeepAliveHost({ activeSessionId, evictRef }: SessionKeepAliveHostProps) {
	const { mountedSessionIds, evictSession } = useSessionKeepAlive(activeSessionId)

	useEffect(() => {
		if (!evictRef) return
		evictRef.current = evictSession
	}, [evictRef, evictSession])

	if (mountedSessionIds.length === 0) return null

	return (
		<>
			{mountedSessionIds.map((id) => {
				const isActive = id === activeSessionId
				return (
					<div
						key={id}
						className={cn(
							"absolute inset-0 h-full transition-opacity duration-150",
							isActive ? "opacity-100" : "pointer-events-none invisible opacity-0",
						)}
						aria-hidden={!isActive}
					>
						<SessionView sessionId={id} isActive={isActive} />
					</div>
				)
			})}
		</>
	)
}
