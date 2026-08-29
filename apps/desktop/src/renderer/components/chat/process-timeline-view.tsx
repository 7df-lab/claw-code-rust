import { Loader2Icon } from "lucide-react"
import { memo, useCallback, type ReactNode } from "react"
import type { ToolPart } from "../../lib/types"
import { ChatToolCall, describeToolGroup, isGroupRunning } from "./chat-tool-call"
import {
	buildProcessTimeline,
	isReasoningPartActivelyStreaming,
	processTimelineRowId,
	type ProcessTimelineInput,
	type ProcessTimelineItem,
} from "./process-timeline"
import { ThoughtRow } from "./thought-row"
import type { ToolCategory } from "./tool-category"
import {
	TranscriptDisclosure,
	TranscriptDisclosureContent,
	TranscriptDisclosureTrigger,
} from "./transcript-disclosure"

export { buildProcessTimeline, isReasoningPartActivelyStreaming }

const TranscriptToolGroupRow = memo(function TranscriptToolGroupRow({
	category,
	tools,
	projectRoot,
	defaultOpen = false,
	open,
	onOpenChange,
	turnWorking = true,
}: {
	category: ToolCategory
	tools: ToolPart[]
	projectRoot?: string | null
	defaultOpen?: boolean
	open?: boolean
	onOpenChange?: (open: boolean) => void
	turnWorking?: boolean
}) {
	const description = describeToolGroup(category, tools, projectRoot)
	const running = isGroupRunning(tools, turnWorking)

	return (
		<TranscriptDisclosure defaultOpen={defaultOpen} open={open} onOpenChange={onOpenChange}>
			<TranscriptDisclosureTrigger
				label={<span>{description}</span>}
				trailing={
					running ? (
						<Loader2Icon className="size-3 animate-spin text-muted-foreground/30" />
					) : undefined
				}
			/>
			<TranscriptDisclosureContent rail className="space-y-0">
				{tools.map((tool) => (
					<ChatToolCall
						key={tool.id}
						part={tool}
						projectRoot={projectRoot}
						turnWorking={turnWorking}
						compact
					/>
				))}
			</TranscriptDisclosureContent>
		</TranscriptDisclosure>
	)
})

export interface ProcessTimelineViewProps {
	items: ProcessTimelineItem[]
	orderedParts: ProcessTimelineInput[]
	working: boolean
	projectRoot?: string | null
	defaultExpandAll?: boolean
	expandedRowIds?: Set<string>
	onToggleRow?: (rowId: string, open: boolean) => void
	renderText: (item: Extract<ProcessTimelineItem, { kind: "text" }>) => ReactNode
	turnHasError?: boolean
	onDeleteToolPart?: (part: ToolPart) => Promise<void>
}

export const ProcessTimelineView = memo(function ProcessTimelineView({
	items,
	orderedParts,
	working,
	projectRoot,
	defaultExpandAll = false,
	expandedRowIds,
	onToggleRow,
	renderText,
	turnHasError,
	onDeleteToolPart,
}: ProcessTimelineViewProps) {
	const resolveOpen = useCallback(
		(rowId: string, fallbackDefault: boolean) => {
			if (defaultExpandAll) return true
			if (expandedRowIds?.has(rowId)) return true
			return fallbackDefault
		},
		[defaultExpandAll, expandedRowIds],
	)

	return (
		<div className="flex flex-col gap-0.5">
			{items.map((item, index) => {
				const rowId = processTimelineRowId(item, index)

				if (item.kind === "text") {
					return <div key={rowId}>{renderText(item)}</div>
				}

				if (item.kind === "thought") {
					const isStreaming = working && isReasoningPartActivelyStreaming(orderedParts, item.part)
					return (
						<ThoughtRow
							key={rowId}
							defaultOpen={defaultExpandAll}
							isStreaming={isStreaming}
							onOpenChange={
								onToggleRow ? (open) => onToggleRow(rowId, open) : undefined
							}
							open={expandedRowIds ? expandedRowIds.has(rowId) : undefined}
							part={item.part}
						/>
					)
				}

				if (item.kind === "tool") {
					return (
						<ChatToolCall
							key={rowId}
							defaultOpen={defaultExpandAll}
							onDelete={onDeleteToolPart}
							open={expandedRowIds ? expandedRowIds.has(rowId) : undefined}
							onOpenChange={
								onToggleRow ? (open) => onToggleRow(rowId, open) : undefined
							}
							part={item.part}
							projectRoot={projectRoot}
							turnHasError={turnHasError}
							turnWorking={working}
						/>
					)
				}

				return (
					<TranscriptToolGroupRow
						key={rowId}
						category={item.category}
						defaultOpen={resolveOpen(rowId, defaultExpandAll)}
						onOpenChange={onToggleRow ? (open) => onToggleRow(rowId, open) : undefined}
						open={expandedRowIds ? expandedRowIds.has(rowId) : undefined}
						projectRoot={projectRoot}
						tools={item.tools}
						turnWorking={working}
					/>
				)
			})}
		</div>
	)
})
