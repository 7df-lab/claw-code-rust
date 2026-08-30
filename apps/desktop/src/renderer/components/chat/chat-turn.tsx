import {
	Message,
	MessageAction,
	MessageActions,
	MessageContent,
	MessageResponse,
} from "@devo/ui/components/ai-elements/message"
import { Shimmer } from "@devo/ui/components/ai-elements/shimmer"
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "@devo/ui/components/dialog"

import {
	ArrowUpToLineIcon,
	BotIcon,
	CheckIcon,
	ChevronDownIcon,
	ChevronRightIcon,
	CopyIcon,
	FileIcon,
	Loader2Icon,
	SplitIcon,
	XIcon,
} from "lucide-react"
import { memo, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react"
import { useDisplayMode } from "../../hooks/use-agents"
import type { SessionCompactionStatus } from "../../atoms/compaction"
import type { ProviderRetryStatus } from "../../atoms/sessions"
import type { ChatMessageEntry, ChatTurn as ChatTurnType } from "../../hooks/use-session-chat"
import {
	computeTurnCost,
	computeTurnWorkTime,
	formatCost,
	formatWorkDuration,
	shortModelName,
} from "../../lib/session-metrics"
import type {
	Agent,
	FilePart,
	Part,
	PermissionRequest,
	PermissionResponse,
	ReasoningPart,
	TextPart,
	ToolPart,
} from "../../lib/types"
import { buildProcessTimeline } from "./process-timeline"
import { ProcessTimelineView } from "./process-timeline-view"
import {
	CompactionStatusDivider,
	compactionStatusFromMetadata,
	isCompactionStatusText,
} from "./compaction-status-divider"
import { PermissionItem } from "./chat-permission"
import { PlanBlock, isPlanTextPart } from "./plan-block"
import { UserMessageBlock } from "./user-message-block"

// ============================================================
// Utility functions
// ============================================================

const DEVO_ITEM_KIND_META = "devo/itemKind"
const DEVO_RESEARCH_ARTIFACT_TITLE_META = "devo/researchArtifactTitle"

/**
 * Formats a timestamp (milliseconds) to relative or absolute time.
 */
export function formatTimestamp(ms: number): string {
	const date = new Date(ms)
	return date.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
}

// ============================================================
// Status computation — follows into sub-agents
// ============================================================

/**
 * Computes a status string from the last active part.
 * Follows into sub-agent sessions for deeper status.
 */
function computeStatus(parts: Part[]): string {
	for (let i = parts.length - 1; i >= 0; i--) {
		const part = parts[i]
		if (part.type === "tool") {
			switch (part.tool) {
				case "task": {
					// Show what the sub-agent is actually doing
					const desc = part.state.input?.description as string | undefined
					const shortDesc = desc && desc.length > 30 ? `${desc.slice(0, 27)}...` : desc
					return shortDesc ? `Agent: ${shortDesc}` : "Delegating..."
				}
				case "todowrite":
				case "todoread":
					return "Planning..."
				case "read":
					return "Reading files..."
				case "list":
				case "grep":
				case "glob":
					return "Searching codebase..."
				case "webfetch":
					return "Fetching web content..."
				case "edit":
				case "write":
				case "apply_patch":
					return "Making edits..."
				case "bash":
					return "Running command..."
				case "question":
					return "Asking a question..."
				default:
					return `Running ${part.tool}...`
			}
		}
		if (part.type === "reasoning") return "Thinking..."
		if (part.type === "text") return "Composing response..."
	}
	return "Working..."
}

// ============================================================
// Synthetic message helpers
// ============================================================

export function isSyntheticMessage(entry: ChatMessageEntry): boolean {
	const textParts = entry.parts.filter((p): p is TextPart => p.type === "text")
	// All text parts are synthetic (e.g. compaction continuation, shell execution)
	if (textParts.length > 0 && textParts.every((p) => p.synthetic === true)) return true
	// No text parts at all — e.g. a user message with only a compaction part
	if (textParts.length === 0 && entry.parts.length > 0) return true
	return false
}

function getUserText(entry: ChatMessageEntry): string {
	return entry.parts
		.filter((p): p is TextPart => p.type === "text" && !p.synthetic)
		.map((p) => p.text)
		.join("\n")
}

function getSyntheticLabel(entry: ChatMessageEntry): string {
	const text = entry.parts
		.filter((p): p is TextPart => p.type === "text")
		.map((p) => p.text)
		.join("\n")
		.toLowerCase()

	if (text.includes("continue if you have next steps")) return "Auto-continued after compaction"
	if (text.includes("summarize the task tool output")) return "Auto-continued after task"
	if (text.includes("tool was executed by the user")) return "Shell command executed"
	if (text.includes("plan has been approved")) return "Plan approved"
	if (text.includes("enter plan mode")) return "Entered plan mode"
	if (text.includes("switch") && text.includes("plan")) return "Mode switched"
	// No text parts — check for compaction part (user message that triggers compaction)
	if (entry.parts.some((p) => p.type === "compaction")) return "Compacting conversation"
	return "Auto-continued"
}

function getFileParts(entry: ChatMessageEntry): FilePart[] {
	return entry.parts.filter(
		(p): p is FilePart =>
			p.type === "file" && (p.mime.startsWith("image/") || p.mime === "application/pdf"),
	)
}

// ============================================================
// Attachment grid
// ============================================================

const AttachmentGrid = memo(function AttachmentGrid({
	files,
	onDelete,
}: { files: FilePart[]; onDelete?: (file: FilePart) => void }) {
	if (files.length === 0) return null
	return (
		<div className="flex flex-wrap gap-2">
			{files.map((file) => (
				<AttachmentThumbnail key={file.id} file={file} onDelete={onDelete} />
			))}
		</div>
	)
})

function AttachmentThumbnail({
	file,
	onDelete,
}: { file: FilePart; onDelete?: (file: FilePart) => void }) {
	const isImage = file.mime.startsWith("image/")
	const [deleting, setDeleting] = useState(false)

	const handleDelete = useCallback(
		async (e: React.MouseEvent) => {
			e.stopPropagation()
			if (!onDelete || deleting) return
			setDeleting(true)
			try {
				await onDelete(file)
			} finally {
				setDeleting(false)
			}
		},
		[onDelete, file, deleting],
	)

	return (
		<Dialog>
			<div className="group/thumb relative size-16 shrink-0">
				{onDelete && (
					<button
						type="button"
						onClick={handleDelete}
						disabled={deleting}
						className="absolute -right-1 -top-1 z-10 flex size-4 items-center justify-center rounded-full bg-destructive text-destructive-foreground opacity-0 shadow-sm transition-opacity hover:bg-destructive/90 group-hover/thumb:opacity-100 disabled:opacity-50"
						title="Remove attachment"
					>
						<XIcon className="size-2.5" />
					</button>
				)}
				<DialogTrigger
					render={
						<button
							type="button"
							className="size-full overflow-hidden rounded-lg border border-border bg-muted transition-colors hover:border-muted-foreground/30"
						/>
					}
				>
					{isImage ? (
						<img
							src={file.url}
							alt={file.filename ?? "Image attachment"}
							className="size-full object-cover"
						/>
					) : (
						<div className="flex size-full items-center justify-center">
							<FileIcon className="size-6 text-muted-foreground" />
						</div>
					)}
					{file.filename && (
						<div className="absolute inset-x-0 bottom-0 bg-black/60 px-1 py-0.5 text-[9px] leading-tight text-white opacity-0 transition-opacity group-hover/thumb:opacity-100">
							<span className="line-clamp-1">{file.filename}</span>
						</div>
					)}
				</DialogTrigger>
			</div>
			<DialogContent className="max-h-[90vh] max-w-4xl overflow-auto p-0">
				<DialogTitle className="sr-only">{file.filename ?? "Attachment preview"}</DialogTitle>
				{isImage ? (
					<img
						src={file.url}
						alt={file.filename ?? "Image attachment"}
						className="max-h-[85vh] w-full object-contain"
					/>
				) : (
					<div className="flex flex-col items-center justify-center gap-2 p-8">
						<FileIcon className="size-12 text-muted-foreground" />
						<p className="text-sm text-muted-foreground">{file.filename ?? "PDF attachment"}</p>
					</div>
				)}
			</DialogContent>
		</Dialog>
	)
}

// ============================================================
// Part extraction helpers
// ============================================================

/** A renderable part — either a tool call, an intermediate text block, or reasoning */
type RenderablePart =
	| { kind: "tool"; part: ToolPart }
	| { kind: "text"; id: string; text: string; metadata?: Record<string, unknown> }
	| { kind: "reasoning"; part: ReasoningPart }

type TextRenderablePart = Extract<RenderablePart, { kind: "text" }>

/**
 * Flattens all assistant parts into an ordered list of renderable items
 * AND extracts the tool-only subset in a single pass.
 * Preserves the natural order: text, reasoning, tool, text, tool, text...
 * Filters out synthetic text, todoread without output, and empty text.
 * Strips OpenRouter [REDACTED] chunks from reasoning and skips empty reasoning.
 */
function getPartsAndTools(assistantMessages: ChatMessageEntry[]): {
	ordered: RenderablePart[]
	tools: ToolPart[]
} {
	const ordered: RenderablePart[] = []
	const tools: ToolPart[] = []
	for (const msg of assistantMessages) {
		for (const part of msg.parts) {
			if (part.type === "tool") {
				tools.push(part)
				if (part.tool === "todoread" && part.state.status !== "completed") continue
				ordered.push({ kind: "tool", part })
			} else if (part.type === "text" && !part.synthetic && part.text.trim()) {
				if (isCompactionStatusText(part.text)) continue
				const metadata = (part as { metadata?: Record<string, unknown> }).metadata
				if (compactionStatusFromMetadata(metadata)) continue
				ordered.push({ kind: "text", id: part.id, text: part.text, metadata })
			} else if (part.type === "reasoning") {
				// Strip OpenRouter's encrypted [REDACTED] chunks
				const cleaned = part.text.replace("[REDACTED]", "").trim()
				if (cleaned) {
					ordered.push({ kind: "reasoning", part })
				}
			}
		}
	}
	return { ordered, tools }
}

function compactionStatusFromTurn(
	assistantMessages: ChatMessageEntry[],
	sessionStatus: SessionCompactionStatus | null | undefined,
): SessionCompactionStatus | null {
	for (const msg of assistantMessages) {
		for (const part of msg.parts) {
			if (part.type !== "text") continue
			const metadata = (part as { metadata?: Record<string, unknown> }).metadata
			const fromPart = compactionStatusFromMetadata(metadata)
			if (fromPart) return fromPart
			if (isCompactionStatusText(part.text)) {
				return sessionStatus === "completed" ? "completed" : "started"
			}
		}
	}
	return null
}

/**
 * Gets the last text part's content — used for the final streaming response
 * and the copy action. Returns undefined if no text parts exist.
 */
function getLastResponseText(orderedParts: RenderablePart[]): string | undefined {
	for (let i = orderedParts.length - 1; i >= 0; i--) {
		const item = orderedParts[i]
		if (item.kind === "text") return item.text
	}
	return undefined
}

function splitCompletedTurnParts(orderedParts: RenderablePart[]): {
	completedProcessParts: RenderablePart[]
	finalResponsePart: TextRenderablePart | undefined
} {
	let finalResponseIndex = -1
	for (let i = orderedParts.length - 1; i >= 0; i--) {
		if (orderedParts[i].kind === "text") {
			finalResponseIndex = i
			break
		}
	}

	if (finalResponseIndex === -1) {
		return { completedProcessParts: orderedParts, finalResponsePart: undefined }
	}

	const finalResponsePart = orderedParts[finalResponseIndex] as TextRenderablePart
	const completedProcessParts = orderedParts.filter((_, index) => index !== finalResponseIndex)
	return { completedProcessParts, finalResponsePart }
}

function researchArtifactTitle(item: TextRenderablePart): string | undefined {
	const metadata = item.metadata
	if (metadata?.[DEVO_ITEM_KIND_META] !== "research_artifact") return undefined
	const title = metadata[DEVO_RESEARCH_ARTIFACT_TITLE_META]
	return typeof title === "string" && title.trim() ? title : undefined
}

function AssistantTextBlock({
	item,
	showPlanActions = false,
	onImplementPlan,
	onRevisePlan,
}: {
	item: TextRenderablePart
	showPlanActions?: boolean
	onImplementPlan?: () => void
	onRevisePlan?: () => void
}) {
	if (isPlanTextPart(item.metadata)) {
		return (
			<PlanBlock
				item={item}
				onImplementPlan={onImplementPlan}
				onRevisePlan={onRevisePlan}
				showActions={showPlanActions}
			/>
		)
	}
	return <ResearchArtifactBlock item={item} />
}

function ResearchArtifactBlock({ item }: { item: TextRenderablePart }) {
	const title = researchArtifactTitle(item)
	if (!title) {
		return (
			<Message from="assistant">
				<MessageContent>
					<MessageResponse>{item.text}</MessageResponse>
				</MessageContent>
			</Message>
		)
	}
	return (
		<div className="border-l border-primary/30 pl-3">
			<div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
				<FileIcon className="size-3" aria-hidden="true" />
				<span>{title}</span>
			</div>
			<Message from="assistant">
				<MessageContent>
					<MessageResponse>{item.text}</MessageResponse>
				</MessageContent>
			</Message>
		</div>
	)
}

function getError(assistantMessages: ChatMessageEntry[]): string | undefined {
	for (const msg of assistantMessages) {
		if (msg.info.role === "assistant" && msg.info.error) {
			const error = msg.info.error
			const errorData = error.data
			// Most error types have a `message` string in data
			if ("message" in errorData && errorData.message) {
				return typeof errorData.message === "string" ? errorData.message : String(errorData.message)
			}
			// Fallback: use the error name (e.g. "MessageOutputLengthError") +
			// any stringifiable data for types like MessageOutputLengthError
			// whose data is { [key: string]: unknown }
			const dataStr = Object.keys(errorData).length > 0 ? JSON.stringify(errorData) : undefined
			return dataStr ? `${error.name}: ${dataStr}` : error.name
		}
	}
	return undefined
}

// ============================================================
// Turn comparison for memo
// ============================================================

/**
 * Lightweight fingerprint for a ChatMessageEntry to detect real content changes
 * without comparing the full object tree. Mirrors the logic in session-chat.ts
 * but kept local to avoid coupling.
 */
function messageEntryFingerprint(entry: ChatMessageEntry): string {
	const lastPart = entry.parts.at(-1)
	const completed = entry.info.role === "assistant" ? (entry.info.time.completed ?? 0) : 0
	let textLen = 0
	const toolSegments: string[] = []
	const textMetadataSegments: string[] = []
	for (const part of entry.parts) {
		if (part.type === "text" || part.type === "reasoning") {
			textLen += part.text.length
			if (part.type === "text") {
				const metadata = (part as { metadata?: Record<string, unknown> }).metadata
				if (metadata?.[DEVO_ITEM_KIND_META] === "research_artifact") {
					textMetadataSegments.push(
						`${part.id}:${metadata[DEVO_ITEM_KIND_META]}:${metadata[DEVO_RESEARCH_ARTIFACT_TITLE_META] ?? ""}`,
					)
				}
				if (
					metadata?.[DEVO_ITEM_KIND_META] === "context_compaction" ||
					metadata?.[DEVO_ITEM_KIND_META] === "proposed_plan" ||
					metadata?.[DEVO_ITEM_KIND_META] === "plan"
				) {
					textMetadataSegments.push(
						`${part.id}:${metadata[DEVO_ITEM_KIND_META]}:${metadata["devo/compactionStatus"] ?? ""}:${part.text.length}`,
					)
				}
			}
		} else if (part.type === "tool") {
			let outLen = 0
			if (part.state.status === "completed") outLen = part.state.output.length
			else if (part.state.status === "error") outLen = part.state.error.length
			else if (part.state.status === "running") {
				const output = part.state.metadata?.output
				outLen = typeof output === "string" ? output.length : 0
			} else if (part.state.status === "pending" && typeof part.state.raw === "string") {
				outLen = part.state.raw.length
			}
			toolSegments.push(`${part.id}:${part.state.status}:${outLen}`)
		}
	}
	return `${entry.info.id}:${completed}:${entry.parts.length}:${lastPart?.id ?? ""}:${textLen}:${textMetadataSegments.join(",")}:${toolSegments.join(",")}`
}

/** Compare two turns by content fingerprint rather than reference equality */
function areTurnsEqual(a: ChatTurnType, b: ChatTurnType): boolean {
	if (a === b) return true
	if (a.id !== b.id) return false
	if (messageEntryFingerprint(a.userMessage) !== messageEntryFingerprint(b.userMessage))
		return false
	if (a.assistantMessages.length !== b.assistantMessages.length) return false
	for (let i = 0; i < a.assistantMessages.length; i++) {
		if (
			messageEntryFingerprint(a.assistantMessages[i]) !==
			messageEntryFingerprint(b.assistantMessages[i])
		)
			return false
	}
	return true
}

// ============================================================
// ChatTurnComponent
// ============================================================

type PendingPermission = {
	request: PermissionRequest
	sessionId: string
}

interface ChatTurnProps {
	turn: ChatTurnType
	isLast: boolean
	isWorking: boolean
	agent?: Agent
	pendingPermission?: PendingPermission
	isConnected?: boolean
	compactionStatus?: SessionCompactionStatus | null
	retryStatus?: ProviderRetryStatus
	onApprovePermission?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		response?: PermissionResponse,
	) => Promise<void>
	onDenyPermission?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
	) => Promise<void>
	/** Fork the conversation from this turn boundary */
	onForkFromTurn?: () => Promise<void>
	/** Edit and resend this turn's user message */
	onEditUserMessage?: (text: string) => Promise<void>
	/** Delete a specific part from a message (for error recovery) */
	onDeletePart?: (sessionId: string, messageId: string, partId: string) => Promise<void>
	onImplementPlan?: () => void
	onRevisePlan?: () => void
}

function pendingPermissionFingerprint(permission: PendingPermission | undefined): string {
	if (!permission) return ""
	const requestId =
		typeof permission.request.id === "string"
			? permission.request.id
			: typeof permission.request.requestID === "string"
				? permission.request.requestID
				: ""
	return `${permission.sessionId}:${requestId}`
}

function retryStatusText(status: ProviderRetryStatus): string {
	if (status.message.trim()) return status.message
	const seconds = Math.max(status.backoffMs / 1000, 0.1)
	return `Retrying provider request in ${seconds.toFixed(1)}s (attempt ${status.attempt})`
}

function WorkingTurnStatusStrip({
	turn,
	retryStatus,
}: {
	turn: ChatTurnType
	retryStatus?: ProviderRetryStatus
}) {
	const [display, setDisplay] = useState(() =>
		formatWorkDuration(computeTurnWorkTime(turn, { active: true })),
	)

	useEffect(() => {
		const updateDisplay = () => {
			setDisplay(formatWorkDuration(computeTurnWorkTime(turn, { active: true })))
		}
		updateDisplay()
		const id = setInterval(updateDisplay, 1_000)
		return () => clearInterval(id)
	}, [turn])

	return (
		<div className="flex items-center gap-2 pt-0.5 text-[13px] leading-5 tabular-nums text-muted-foreground">
			<span aria-hidden="true" className="size-1.5 shrink-0 animate-pulse rounded-full bg-foreground/45" />
			{retryStatus ? retryStatusText(retryStatus) : <>Working for {display}</>}
		</div>
	)
}

function CompletedTurnProcessDisclosure({
	duration,
	expanded,
	hasProcessDetails,
	onToggle,
}: {
	duration: string
	expanded: boolean
	hasProcessDetails: boolean
	onToggle: () => void
}) {
	const label = (
		<span>
			{duration ? "Worked for " : "Worked"}
			{duration}
		</span>
	)
	const chevron = hasProcessDetails ? (
		expanded ? (
			<ChevronDownIcon
				aria-hidden="true"
				className="size-3.5 shrink-0 text-muted-foreground/70"
			/>
		) : (
			<ChevronRightIcon
				aria-hidden="true"
				className="size-3.5 shrink-0 text-muted-foreground/70 opacity-0 transition-opacity group-hover/worked:opacity-100 group-focus-visible/worked:opacity-100"
			/>
		)
	) : null

	if (!hasProcessDetails) {
		return (
			<div className="flex w-full max-w-full items-center gap-2 py-0.5 text-[13px] leading-5 tabular-nums text-muted-foreground">
				{label}
			</div>
		)
	}

	return (
		<button
			type="button"
			onClick={onToggle}
			aria-expanded={expanded}
			className="group/worked flex w-fit max-w-full items-center gap-0.5 py-0.5 text-left text-[13px] leading-5 tabular-nums text-muted-foreground transition-colors hover:text-foreground"
		>
			<span className="min-w-0 truncate">{label}</span>
			{chevron}
		</button>
	)
}

/**
 * Renders a single turn: user message + assistant response.
 *
 * Two modes based on turn state:
 * - **Active turn** (last + working): the process timeline streams live —
 *   interleaved thoughts, unified tool rows, and text.
 * - **Completed turn**: the process collapses behind a "Worked for ..."
 *   disclosure; the final response text is always visible.
 *
 * Display mode preference (default/verbose) modifies behavior:
 * - default: interleaved text + grouped tool summaries as collapsible rows.
 * - verbose: all turns show all tools expanded with full content.
 */
export const ChatTurnComponent = memo(
	function ChatTurnComponent({
		turn,
		isLast,
		isWorking,
		agent,
		pendingPermission,
		isConnected = false,
		compactionStatus,
		retryStatus,
		onApprovePermission,
		onDenyPermission,
		onForkFromTurn,
		onEditUserMessage,
		onDeletePart,
		onImplementPlan,
		onRevisePlan,
	}: ChatTurnProps) {
		const [completedProcessExpanded, setCompletedProcessExpanded] = useState(false)
		const [expandedRowIds, setExpandedRowIds] = useState<Set<string>>(() => new Set())
		const [copied, setCopied] = useState(false)
		const displayMode = useDisplayMode()
		const toolPathRoot = agent?.worktreePath ?? agent?.directory ?? agent?.projectDirectory
		const turnRef = useRef<HTMLDivElement>(null)
		useEffect(() => {
			setCompletedProcessExpanded(false)
			setExpandedRowIds(new Set())
		}, [turn.id])

		const isSynthetic = useMemo(() => isSyntheticMessage(turn.userMessage), [turn.userMessage])
		const userText = useMemo(() => getUserText(turn.userMessage), [turn.userMessage])
		const syntheticLabel = useMemo(
			() => (isSynthetic ? getSyntheticLabel(turn.userMessage) : ""),
			[isSynthetic, turn.userMessage],
		)
		const userFiles = useMemo(() => getFileParts(turn.userMessage), [turn.userMessage])

		// Ordered parts + tool-only subset in a single pass (avoids double iteration)
		const { ordered: orderedParts } = useMemo(
			() => getPartsAndTools(turn.assistantMessages),
			[turn.assistantMessages],
		)

		const { completedProcessParts, finalResponsePart } = useMemo(
			() => splitCompletedTurnParts(orderedParts),
			[orderedParts],
		)
		const displayedCompactionStatus: SessionCompactionStatus | null = compactionStatusFromTurn(
			turn.assistantMessages,
			compactionStatus,
		)

		// The last text for streaming display and copy action
		const rawResponseText = useMemo(() => getLastResponseText(orderedParts), [orderedParts])
		const responseText = useDeferredValue(rawResponseText)

		const errorText = useMemo(() => getError(turn.assistantMessages), [turn.assistantMessages])

		// Compute status by walking the last message's parts in reverse — no
		// need to flatMap all messages into a temporary array.
		const statusText = useMemo(() => {
			if (retryStatus) return retryStatusText(retryStatus)
			for (let m = turn.assistantMessages.length - 1; m >= 0; m--) {
				const status = computeStatus(turn.assistantMessages[m].parts)
				if (status !== "Working...") return status
			}
			return "Working..."
		}, [retryStatus, turn.assistantMessages])

		const working = isLast && isWorking

		// User requirement: queue state belongs in the composer status stack;
		// this transcript must not infer queued state from an empty assistant response.
		const processOrderedParts = working ? orderedParts : completedProcessParts
		const processTimelineItems = useMemo(
			() => buildProcessTimeline(processOrderedParts),
			[processOrderedParts],
		)
		const processToolParts = useMemo(
			() => processOrderedParts.flatMap((part) => (part.kind === "tool" ? [part.part] : [])),
			[processOrderedParts],
		)
		const hasSteps = processToolParts.length > 0
		const hasWorkToDisclose = !working && processTimelineItems.length > 0
		const hasCompletedProcessDetails = hasWorkToDisclose
		const workTimeMs = useMemo(
			() => computeTurnWorkTime(turn, { active: working }),
			[turn, working],
		)
		const showWorkedForSummary = useMemo(() => {
			if (working) return false
			return turn.assistantMessages.length > 0
		}, [turn.assistantMessages.length, working])
		const processSectionVisible =
			(working && processTimelineItems.length > 0) ||
			(!working && hasCompletedProcessDetails && completedProcessExpanded)

		const duration = useMemo(() => {
			if (workTimeMs <= 0) return ""
			return formatWorkDuration(workTimeMs)
		}, [workTimeMs])
		const turnCostStr = useMemo(() => {
			const cost = computeTurnCost(turn)
			return cost > 0 ? formatCost(cost) : ""
		}, [turn])
		const turnModel = useMemo(() => {
			for (let i = turn.assistantMessages.length - 1; i >= 0; i--) {
				const info = turn.assistantMessages[i].info
				if (info.role === "assistant" && info.modelID) {
					return shortModelName(info.modelID)
				}
			}
			return ""
		}, [turn.assistantMessages])

		const showVerboseTools = displayMode === "verbose"

		const textAlreadyInline =
			processSectionVisible && processOrderedParts.some((p) => p.kind === "text")

		useEffect(() => {
			if (working) return
			setCompletedProcessExpanded(false)
			setExpandedRowIds(new Set())
		}, [working])

		const handleToggleTimelineRow = useCallback((rowId: string, open: boolean) => {
			setExpandedRowIds((previous) => {
				const next = new Set(previous)
				if (open) next.add(rowId)
				else next.delete(rowId)
				return next
			})
		}, [])

		const handleCopyResponse = useCallback(async () => {
			if (!responseText) return
			await navigator.clipboard.writeText(responseText)
			setCopied(true)
			setTimeout(() => setCopied(false), 2000)
		}, [responseText])

		const handleScrollToTop = useCallback(() => {
			turnRef.current?.scrollIntoView({ behavior: "smooth", block: "start" })
		}, [])

		const handleToggleCompletedProcess = useCallback(() => {
			setCompletedProcessExpanded((expanded) => !expanded)
		}, [])

		const [forking, setForking] = useState(false)
		const handleFork = useCallback(async () => {
			if (!onForkFromTurn || forking) return
			setForking(true)
			try {
				await onForkFromTurn()
			} finally {
				setForking(false)
			}
		}, [onForkFromTurn, forking])

		const handleDeleteFile = useCallback(
			async (file: FilePart) => {
				if (!onDeletePart) return
				await onDeletePart(file.sessionID, file.messageID, file.id)
			},
			[onDeletePart],
		)

		const handleDeleteToolPart = useCallback(
			async (toolPart: ToolPart) => {
				if (!onDeletePart) return
				await onDeletePart(toolPart.sessionID, toolPart.messageID, toolPart.id)
			},
			[onDeletePart],
		)

		return (
			<div ref={turnRef} className="group/turn space-y-3">
				{/* User message */}
				{isSynthetic ? (
					<div className="flex items-center justify-end gap-1.5 text-[11px] italic text-muted-foreground/50">
						<BotIcon className="size-3" aria-hidden="true" />
						<span>{syntheticLabel}</span>
					</div>
				) : (
					<UserMessageBlock
						text={userText}
						canEdit={!!onEditUserMessage}
						onEdit={onEditUserMessage}
					>
						{userFiles.length > 0 && (
							<AttachmentGrid
								files={userFiles}
								onDelete={onDeletePart ? handleDeleteFile : undefined}
							/>
						)}
					</UserMessageBlock>
				)}

				{working && <WorkingTurnStatusStrip turn={turn} retryStatus={retryStatus} />}

				{!working && showWorkedForSummary && (
					<CompletedTurnProcessDisclosure
						duration={duration}
						expanded={completedProcessExpanded}
						hasProcessDetails={hasCompletedProcessDetails}
						onToggle={handleToggleCompletedProcess}
					/>
				)}

				{/* Interleaved thought/tool process timeline */}
				{processSectionVisible && (
					<div className="flex flex-col gap-1">
						<ProcessTimelineView
							defaultExpandAll={showVerboseTools}
							expandedRowIds={showVerboseTools ? undefined : expandedRowIds}
							items={processTimelineItems}
							onDeleteToolPart={onDeletePart ? handleDeleteToolPart : undefined}
							onToggleRow={showVerboseTools ? undefined : handleToggleTimelineRow}
							orderedParts={processOrderedParts}
							projectRoot={toolPathRoot}
							renderText={(item) => (
								<div className="py-0.5">
									<AssistantTextBlock
										item={item}
										onImplementPlan={onImplementPlan}
										onRevisePlan={onRevisePlan}
										showPlanActions={isLast && !working}
									/>
								</div>
							)}
							turnHasError={!!errorText}
							working={working}
						/>

						{working && hasSteps && (
							<div className="flex items-center gap-2 text-[13px] text-muted-foreground">
								<Loader2Icon className="size-3 animate-spin text-muted-foreground/30" />
								<Shimmer className="text-[11px]">{statusText}</Shimmer>
							</div>
						)}
					</div>
				)}

				{pendingPermission && agent && (
					<PermissionItem
						agent={agent}
						permission={pendingPermission.request}
						onApprove={onApprovePermission}
						onDeny={onDenyPermission}
						isConnected={isConnected}
						isFromSubAgent={pendingPermission.sessionId !== agent.sessionId}
					/>
				)}

				{/* Error */}
				{errorText && (
					<div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
						{errorText.length > 300 ? `${errorText.slice(0, 300)}...` : errorText}
					</div>
				)}

				{/* Completed final response */}
				{!working && finalResponsePart && responseText && (
					<div>
					<AssistantTextBlock
						item={{ ...finalResponsePart, text: responseText }}
						onImplementPlan={onImplementPlan}
						onRevisePlan={onRevisePlan}
						showPlanActions={isLast}
					/>
					</div>
				)}

				{/* Streaming response — visible while working, when text isn't already inline */}
				{working && responseText && !textAlreadyInline && (
					<div>
					{isPlanTextPart(finalResponsePart?.metadata) ? (
						<AssistantTextBlock item={{ ...(finalResponsePart as TextRenderablePart), text: responseText }} />
					) : (
						<Message from="assistant">
							<MessageContent>
								<MessageResponse animated>{responseText}</MessageResponse>
							</MessageContent>
						</Message>
					)}
					</div>
				)}

				{/* User requirement: render compaction lifecycle as a transcript divider,
				   not as a normal assistant message that can hide the previous reply. */}
				{displayedCompactionStatus && (
					<CompactionStatusDivider status={displayedCompactionStatus} />
				)}

				{/* Per-turn metadata — shown on completed turns so badges are visible after long responses */}
				{!working && turn.assistantMessages.length > 0 && (turnModel || turnCostStr) && (
					<div className="flex items-center gap-1.5 text-[11px] tabular-nums text-muted-foreground/45">
						{turnModel && <span>{turnModel}</span>}
						{turnModel && turnCostStr && <span>·</span>}
						{turnCostStr && <span>{turnCostStr}</span>}
					</div>
				)}

				{/* Turn-level message actions — only after the assistant turn finishes */}
				{!working && responseText && (
					<MessageActions className="-ml-1">
						<MessageAction tooltip="Scroll to top" onClick={handleScrollToTop}>
							<ArrowUpToLineIcon className="size-3" />
						</MessageAction>
						<MessageAction
							tooltip={copied ? "Copied" : "Copy response"}
							onClick={handleCopyResponse}
						>
							{copied ? <CheckIcon className="size-3" /> : <CopyIcon className="size-3" />}
						</MessageAction>
					{onForkFromTurn && !working && (
						<MessageAction
							tooltip={forking ? "Forking..." : "Fork from here"}
							onClick={handleFork}
							disabled={forking}
						>
							<SplitIcon className="size-3" />
						</MessageAction>
					)}
					</MessageActions>
				)}
			</div>
		)
	},
	(prev, next) => {
		if (!areTurnsEqual(prev.turn, next.turn)) return false
		if (prev.isLast !== next.isLast) return false
		if (prev.isWorking !== next.isWorking) return false
		if (prev.retryStatus !== next.retryStatus) return false
		if (prev.agent?.sessionId !== next.agent?.sessionId) return false
		if (prev.agent?.directory !== next.agent?.directory) return false
		if (prev.agent?.projectDirectory !== next.agent?.projectDirectory) return false
		if (prev.agent?.worktreePath !== next.agent?.worktreePath) return false
		if (prev.isConnected !== next.isConnected) return false
		if (prev.compactionStatus !== next.compactionStatus) return false
		if (
			pendingPermissionFingerprint(prev.pendingPermission) !==
			pendingPermissionFingerprint(next.pendingPermission)
		) {
			return false
		}
		// Skip reference comparison for callbacks - they close over stable values
		// and their identity changes don't affect rendered output
		return true
	},
)
