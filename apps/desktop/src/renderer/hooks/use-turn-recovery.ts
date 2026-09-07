import { useCallback, useEffect, useRef, useState } from "react"
import { getProjectClient } from "../services/connection-manager"

export interface TurnRecoveryInfo {
    turnId: string
    revision: number
    attempt: number
    reason: string
}

/** Recovery is server-owned; a historical running label never authorizes replay. */
export function useTurnRecovery(sessionId: string, directory: string, status: string) {
    const [recovery, setRecovery] = useState<TurnRecoveryInfo | null>(null)
    const [pending, setPending] = useState(false)
    const [error, setError] = useState<string | null>(null)
    const requestKey = useRef<string | null>(null)
    const generation = useRef(0)
    const scope = useRef(0)
    const inFlight = useRef(false)
    const client = getProjectClient(directory)
    const refresh = useCallback(async () => {
        const request = ++generation.current
        try {
            const result = await client.turnRecovery.read(sessionId) as { recovery: TurnRecoveryInfo | null }
            if (request === generation.current) setRecovery(result.recovery)
        } catch (cause) {
            if (request === generation.current) setError(String(cause))
        }
    }, [client, sessionId])

    useEffect(() => {
        scope.current++
        inFlight.current = false
        setPending(false)
        setRecovery(null)
        setError(null)
        requestKey.current = null
        void refresh()
        let retry: ReturnType<typeof setTimeout> | undefined
        const update = () => {
            clearTimeout(retry)
            // Turn completion is observed just before the actor merge completes.
            retry = setTimeout(() => { void refresh() }, 250)
        }
        const unsubscribe = client.turnRecovery.subscribe(update)
        window.addEventListener("focus", update)
        return () => {
            scope.current++
            generation.current++
            clearTimeout(retry)
            unsubscribe()
            window.removeEventListener("focus", update)
        }
    }, [client, refresh])

    useEffect(() => { void refresh() }, [status, refresh])

    const resolve = useCallback(async (action: "continue" | "cancel") => {
        if (!recovery || inFlight.current) return
        const requestScope = scope.current
        inFlight.current = true
        setPending(true)
        setError(null)
        try {
            if (action === "continue") {
                requestKey.current ??= crypto.randomUUID()
                await client.turnRecovery.resume(sessionId, recovery, requestKey.current)
            } else {
                await client.turnRecovery.cancel(sessionId)
            }
            if (requestScope === scope.current) {
                setRecovery(null)
                requestKey.current = null
            }
        } catch (cause) {
            if (requestScope === scope.current) setError(String(cause))
        } finally {
            if (requestScope === scope.current) {
                inFlight.current = false
                setPending(false)
                await refresh()
            }
        }
    }, [client, sessionId, recovery, refresh])

    return { recovery, pending, error, resolve }
}
