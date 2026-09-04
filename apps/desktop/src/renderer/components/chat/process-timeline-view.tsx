import { memo, useCallback, type ReactNode } from "react"
import type { ToolPart } from "../../lib/types"
import { ChatToolCall, describeToolGroup } from "./chat-tool-call"
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

	return (
		<TranscriptDisclosure defaultOpen={defaultOpen} open={open} onOpenChange={onOpenChange}>
			<TranscriptDisclosureTrigger label={<span>{description}</span>} />
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

const ProcessTimelineTextRow = memo(function ProcessTimelineTextRow({
	children,
}: {
	children: ReactNode
}) {
	return <div>{children}</div>
})

const ProcessTimelineThoughtRow = memo(function ProcessTimelineThoughtRow({
	rowId,
	part,
	isStreaming,
	defaultExpandAll,
	expanded,
	onToggleRow,
}: {
	rowId: string
	part: Extract<ProcessTimelineItem, { kind: "thought" }>["part"]
	isStreaming: boolean
	defaultExpandAll: boolean
	expanded?: boolean
	onToggleRow?: (rowId: string, open: boolean) => void
}) {
	const handleOpenChange = useCallback(
		(open: boolean) => {
			onToggleRow?.(rowId, open)
		},
		[onToggleRow, rowId],
	)

	return (
		<ThoughtRow
			defaultOpen={defaultExpandAll}
			isStreaming={isStreaming}
			onOpenChange={onToggleRow ? handleOpenChange : undefined}
			open={expanded}
			part={part}
		/>
	)
})

const ProcessTimelineToolRow = memo(function ProcessTimelineToolRow({
	rowId,
	part,
	defaultExpandAll,
	expanded,
	onToggleRow,
	onDeleteToolPart,
	projectRoot,
	turnHasError,
	working,
}: {
	rowId: string
	part: ToolPart
	defaultExpandAll: boolean
	expanded?: boolean
	onToggleRow?: (rowId: string, open: boolean) => void
	onDeleteToolPart?: (part: ToolPart) => Promise<void>
	projectRoot?: string | null
	turnHasError?: boolean
	working: boolean
}) {
	const handleOpenChange = useCallback(
		(open: boolean) => {
			onToggleRow?.(rowId, open)
		},
		[onToggleRow, rowId],
	)

	return (
		<ChatToolCall
			defaultOpen={defaultExpandAll}
			onDelete={onDeleteToolPart}
			open={expanded}
			onOpenChange={onToggleRow ? handleOpenChange : undefined}
			part={part}
			projectRoot={projectRoot}
			turnHasError={turnHasError}
			turnWorking={working}
		/>
	)
})

const ProcessTimelineToolGroupRow = memo(function ProcessTimelineToolGroupRow({
	rowId,
	category,
	tools,
	defaultOpen,
	expanded,
	onToggleRow,
	projectRoot,
	working,
}: {
	rowId: string
	category: ToolCategory
	tools: ToolPart[]
	defaultOpen: boolean
	expanded?: boolean
	onToggleRow?: (rowId: string, open: boolean) => void
	projectRoot?: string | null
	working: boolean
}) {
	const handleOpenChange = useCallback(
		(open: boolean) => {
			onToggleRow?.(rowId, open)
		},
		[onToggleRow, rowId],
	)

	return (
		<TranscriptToolGroupRow
			category={category}
			defaultOpen={defaultOpen}
			onOpenChange={onToggleRow ? handleOpenChange : undefined}
			open={expanded}
			projectRoot={projectRoot}
			tools={tools}
			turnWorking={working}
		/>
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
					return (
						<ProcessTimelineTextRow key={rowId}>{renderText(item)}</ProcessTimelineTextRow>
					)
				}

				if (item.kind === "thought") {
					const isStreaming = working && isReasoningPartActivelyStreaming(orderedParts, item.part)
					return (
						<ProcessTimelineThoughtRow
							key={rowId}
							defaultExpandAll={defaultExpandAll}
							expanded={expandedRowIds ? expandedRowIds.has(rowId) : undefined}
							isStreaming={isStreaming}
							onToggleRow={onToggleRow}
							part={item.part}
							rowId={rowId}
						/>
					)
				}

				if (item.kind === "tool") {
					return (
						<ProcessTimelineToolRow
							key={rowId}
							defaultExpandAll={defaultExpandAll}
							expanded={expandedRowIds ? expandedRowIds.has(rowId) : undefined}
							onDeleteToolPart={onDeleteToolPart}
							onToggleRow={onToggleRow}
							part={item.part}
							projectRoot={projectRoot}
							rowId={rowId}
							turnHasError={turnHasError}
							working={working}
						/>
					)
				}

				return (
					<ProcessTimelineToolGroupRow
						key={rowId}
						category={item.category}
						defaultOpen={resolveOpen(rowId, defaultExpandAll)}
						expanded={expandedRowIds ? expandedRowIds.has(rowId) : undefined}
						onToggleRow={onToggleRow}
						projectRoot={projectRoot}
						rowId={rowId}
						tools={item.tools}
						working={working}
					/>
				)
			})}
		</div>
	)
})
