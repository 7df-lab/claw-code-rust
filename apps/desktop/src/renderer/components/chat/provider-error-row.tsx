/**
 * Expandable provider/LLM error row — same disclosure chrome as tool calls.
 * Scheduled retries show a live countdown; expand reveals the failure reason.
 */
import { memo, useEffect, useState } from "react"
import type { ProviderErrorEntry } from "../../atoms/sessions"
import {
	TranscriptDisclosure,
	TranscriptDisclosureContent,
	TranscriptDisclosureTrigger,
} from "./transcript-disclosure"

export type { ProviderErrorEntry }

function remainingBackoffMs(entry: ProviderErrorEntry, nowMs: number): number {
	if (entry.phase !== "scheduled") return 0
	const backoff = entry.backoffMs ?? 0
	if (backoff <= 0) return 0
	const started = entry.scheduledAtMs ?? nowMs
	return Math.max(0, backoff - (nowMs - started))
}

function formatCountdown(ms: number): string {
	const totalSeconds = ms / 1000
	if (totalSeconds >= 10) return `${Math.ceil(totalSeconds)}s`
	return `${totalSeconds.toFixed(1)}s`
}

function summaryLabel(entry: ProviderErrorEntry, remainingMs: number, pending: boolean): string {
	if (entry.phase === "scheduled") {
		const attempt =
			entry.attempt != null && entry.attempt > 0 ? ` (attempt ${entry.attempt})` : ""
		if (pending && remainingMs > 0) {
			return `Provider retry${attempt} · ${formatCountdown(remainingMs)}`
		}
		return `Provider retry${attempt}`
	}
	if (entry.code && entry.code !== "Error" && entry.code !== "TurnFailed") {
		return entry.code
	}
	return "Request failed"
}

export const ProviderErrorRow = memo(function ProviderErrorRow({
	entry,
	pending = false,
}: {
	entry: ProviderErrorEntry
	pending?: boolean
}) {
	const [nowMs, setNowMs] = useState(() => Date.now())
	const remainingMs = remainingBackoffMs(entry, nowMs)
	const liveCountdown = pending && entry.phase === "scheduled" && (entry.backoffMs ?? 0) > 0

	useEffect(() => {
		if (!liveCountdown) return
		setNowMs(Date.now())
		const id = window.setInterval(() => setNowMs(Date.now()), 100)
		return () => window.clearInterval(id)
	}, [liveCountdown, entry.id, entry.scheduledAtMs, entry.backoffMs])

	const label = summaryLabel(entry, remainingMs, pending)
	return (
		<TranscriptDisclosure defaultOpen={false}>
			<TranscriptDisclosureTrigger
				label={<span className="tabular-nums">{label}</span>}
				aria-label={`${label}: expand to view details`}
			/>
			<TranscriptDisclosureContent rail className="space-y-1">
				{entry.code ? (
					<p className="font-mono text-[11px] text-muted-foreground/70">{entry.code}</p>
				) : null}
				<pre className="whitespace-pre-wrap break-words font-sans text-[13px] leading-5 text-muted-foreground">
					{entry.message}
				</pre>
			</TranscriptDisclosureContent>
		</TranscriptDisclosure>
	)
})
