import { ReasoningText } from "@devo/ui/components/ai-elements/reasoning"
import { Shimmer } from "@devo/ui/components/ai-elements/shimmer"
import { memo, useEffect, useState } from "react"
import {
	computeThoughtWorkTime,
	formatWorkDuration,
} from "../../lib/session-metrics"
import type { ReasoningPart } from "../../lib/types"
import {
	TranscriptDisclosure,
	TranscriptDisclosureContent,
	TranscriptDisclosureTrigger,
} from "./transcript-disclosure"

export const ThoughtRow = memo(function ThoughtRow({
	part,
	isStreaming,
	open,
	defaultOpen = false,
	onOpenChange,
}: {
	part: ReasoningPart
	isStreaming: boolean
	open?: boolean
	defaultOpen?: boolean
	onOpenChange?: (open: boolean) => void
}) {
	const text = part.text.replace("[REDACTED]", "").trim()
	if (!text) return null

	return (
		<TranscriptDisclosure
			defaultOpen={defaultOpen}
			open={open}
			onOpenChange={onOpenChange}
		>
			<TranscriptDisclosureTrigger
				aria-label="Reasoning details"
				label={<ThoughtLabel isStreaming={isStreaming} part={part} />}
			/>
			<TranscriptDisclosureContent rail>
				<div
					aria-label="Reasoning details"
					className="text-[13px] leading-5 text-muted-foreground/80 [&>*:first-child]:mt-0 [&>*:last-child]:mb-0 [&_p]:my-0"
				>
					<ReasoningText animated={isStreaming}>{text}</ReasoningText>
				</div>
			</TranscriptDisclosureContent>
		</TranscriptDisclosure>
	)
})

function ThoughtLabel({
	part,
	isStreaming,
}: {
	part: ReasoningPart
	isStreaming: boolean
}) {
	const [display, setDisplay] = useState(() =>
		formatThoughtDuration(part, isStreaming),
	)

	useEffect(() => {
		const update = () => setDisplay(formatThoughtDuration(part, isStreaming))
		update()
		if (!isStreaming) return
		const id = setInterval(update, 1_000)
		return () => clearInterval(id)
	}, [part, isStreaming])

	if (isStreaming) {
		return (
			<span className="inline-flex items-baseline gap-1 tabular-nums">
				<Shimmer duration={1}>Thinking...</Shimmer>
				{display ? <span>{display}</span> : null}
			</span>
		)
	}

	return <span className="tabular-nums">{display || "Thought"}</span>
}

function formatThoughtDuration(part: ReasoningPart, isStreaming: boolean): string {
	const ms = computeThoughtWorkTime(part, { active: isStreaming })
	if (ms <= 0) return ""
	const duration = formatWorkDuration(ms)
	return isStreaming ? duration : `Thought for ${duration}`
}
