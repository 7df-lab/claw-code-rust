import { ReasoningText } from "@devo/ui/components/ai-elements/reasoning"
import { Shimmer } from "@devo/ui/components/ai-elements/shimmer"
import { memo } from "react"
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
				label={
					isStreaming ? (
						<Shimmer duration={1}>Thinking...</Shimmer>
					) : (
						<span>Thought</span>
					)
				}
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
