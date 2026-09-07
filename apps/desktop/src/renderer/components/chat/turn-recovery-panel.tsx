import type { useTurnRecovery } from "../../hooks/use-turn-recovery"

export function TurnRecoveryPanel({ state }: { state: ReturnType<typeof useTurnRecovery> }) {
    if (!state.recovery) return null
    return (
        <section aria-label="Interrupted turn recovery" className="mb-2 rounded-xl border border-border bg-background p-3 text-sm">
            <p>This turn stopped unexpectedly. Continue from the saved context?</p>
            {state.recovery.reason && <p className="mt-1 text-xs text-muted-foreground">{state.recovery.reason}</p>}
            {state.error && <p role="alert" className="mt-1 text-xs text-destructive">{state.error}</p>}
            <div className="mt-2 flex gap-2">
                <button type="button" disabled={state.pending} onClick={() => void state.resolve("continue")}
                    className="rounded-md bg-primary px-3 py-1 text-primary-foreground disabled:opacity-50">Continue</button>
                <button type="button" disabled={state.pending} onClick={() => void state.resolve("cancel")}
                    className="rounded-md border px-3 py-1 disabled:opacity-50">Cancel</button>
            </div>
        </section>
    )
}
