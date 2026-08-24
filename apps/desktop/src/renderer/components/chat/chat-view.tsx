import {
	Conversation,
	ConversationContent,
	ConversationScrollButton,
	useStickToBottomContext,
} from "@devo/ui/components/ai-elements/conversation"
import {
	PromptInput,
	PromptInputButton,
	PromptInputFooter,
	PromptInputProvider,
	PromptInputSubmit,
	PromptInputTextarea,
	PromptInputTools,
	usePromptInputAttachments,
	usePromptInputController,
} from "@devo/ui/components/ai-elements/prompt-input"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { cn } from "@devo/ui/lib/utils"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useAtom, useAtomValue, useSetAtom } from "jotai"
import {
	ArrowUpToLineIcon,
	GitForkIcon,
	GoalIcon,
	ListTodoIcon,
	Loader2Icon,
	PlusIcon,
	Redo2Icon,
	SquareIcon,
	Undo2Icon,
	XIcon,
} from "lucide-react"
import {
	type CSSProperties,
	type ReactNode,
	type RefObject,
	useCallback,
	useEffect,
	useImperativeHandle,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react"
import { collaborationModeFamily } from "../../atoms/collaboration-mode"
import { compactionStatusFamily } from "../../atoms/compaction"
import { messagesFamily } from "../../atoms/messages"
import { projectModelsAtom, setProjectModelAtom } from "../../atoms/preferences"
import type { ProviderRetryStatus, SessionSetupPhase } from "../../atoms/sessions"
import { removePermissionAtom, sessionFamily } from "../../atoms/sessions"
import {
	effectivePermissionFamily,
	effectiveQuestionFamily,
} from "../../atoms/derived/session-requests"
import { appStore } from "../../atoms/store"
import { sessionScrollTopFamily, settingsOverlayOpenAtom } from "../../atoms/ui"
import { useDraftActions, useDraftSnapshot } from "../../hooks/use-draft"
import {
	freezeSessionScroll,
	getFrozenSessionScroll,
	getPendingRestoreScrollTop,
	getRestoredScrollTop,
	markScrollRestored,
	setPendingRestoreScrollTop,
	trackSettingsOverlayOpen,
} from "../../lib/settings-scroll-freeze"
import type {
	ConfigData,
	ModelRef,
	ProvidersData,
	SdkAgent,
	VcsData,
} from "../../hooks/use-devo-data"
import {
	getModelInputCapabilities,
	getModelVariants,
	resolveEffectiveModel,
	useModelState,
} from "../../hooks/use-devo-data"
import type { ChatTurn } from "../../hooks/use-session-chat"
import { createLogger } from "../../lib/logger"
import { computeTurnWorkTimeSplit, formatWorkDuration } from "../../lib/session-metrics"
import type { Agent, FileAttachment, PermissionResponse, QuestionAnswer } from "../../lib/types"
import { persistRuntimeModelConfigOption, persistRuntimeModelSelection } from "../../lib/model-config-options"
import { getProjectClient } from "../../services/connection-manager"

const log = createLogger("chat-view")

const VIRTUALIZE_TURN_THRESHOLD = 30
const VIRTUAL_TURN_GAP = 40

import {
	type DiffComment,
	diffCommentsFamily,
	serializeCommentsForChat,
} from "../review/review-comments"
import { PermissionItem } from "./chat-permission"
import { ChatQuestionFlow } from "./chat-question"
import { ChatTurnComponent } from "./chat-turn"
import {
	ComposerStatusStack,
	type ComposerGoal,
	type ComposerGoalStatus,
} from "./composer-status-stack"
import { ContextItems } from "./context-items"
import type { MentionOption } from "./mention-popover"
import { MentionPopover, type MentionPopoverHandle } from "./mention-popover"
import { PromptAttachmentPreview } from "./prompt-attachments"
import {
	createMentionFromOption,
	getMentionKey,
	getMentionMarker,
	insertMentionIntoText,
	type PromptMention,
	reconcileMentions,
} from "./prompt-mentions"
import { PromptToolbar } from "./prompt-toolbar"
import { SessionTaskList } from "./session-task-list"
import { SkillPickerDialog } from "./skill-picker-dialog"
import { SlashCommandPopover, type SlashCommandPopoverHandle } from "./slash-command-popover"

type ComposerTrigger = "goal" | "plan"
type ComposerGoalAction = "edit" | "pause" | "resume" | "clear"

function objectRecord(value: unknown): Record<string, unknown> | null {
	return value && typeof value === "object" ? (value as Record<string, unknown>) : null
}

function normalizeGoalStatus(value: unknown): ComposerGoalStatus | null {
	if (value === "active" || value === "paused" || value === "complete") return value
	if (value === "budgetLimited" || value === "budget_limited") return "budgetLimited"
	return null
}

function normalizeComposerGoal(value: unknown): ComposerGoal | null {
	const record = objectRecord(value)
	if (!record) return null
	const objective = typeof record.objective === "string" ? record.objective.trim() : ""
	const status = normalizeGoalStatus(record.status)
	if (!objective || !status || status === "complete") return null
	return {
		objective,
		status,
		timeUsedSeconds: (record.timeUsedSeconds ?? record.time_used_seconds) as ComposerGoal["timeUsedSeconds"],
		observedAtMs: Date.now(),
	}
}

/**
 * Small "+" button that opens the file picker for attachments.
 * Must be rendered inside a <PromptInput> so the attachments context is available.
 */
function AttachButton({ disabled }: { disabled?: boolean }) {
	const attachments = usePromptInputAttachments()
	return (
		<PromptInputButton
			tooltip="Attach files"
			onClick={() => attachments.openFileDialog()}
			disabled={disabled}
		>
			<PlusIcon className="size-4" />
		</PromptInputButton>
	)
}

function ComposerTriggerChip({
	trigger,
	onRemove,
}: {
	trigger: ComposerTrigger
	onRemove: () => void
}) {
	const isPlan = trigger === "plan"
	const Icon = isPlan ? ListTodoIcon : GoalIcon
	const label = isPlan ? "Plan" : "Goal"
	const description = isPlan ? "Create a plan" : "Set a goal"

	return (
		<Tooltip>
			<TooltipTrigger
				render={
					<div className="group inline-flex h-7 items-center gap-1 rounded-full px-2 text-xs text-muted-foreground transition-colors hover:bg-muted hover:text-foreground" />
				}
			>
				<button
					type="button"
					aria-label={`Remove ${label} trigger`}
					onClick={onRemove}
					// User requirement: hover replaces the trigger icon in-place with
					// the close affordance, so the chip text never shifts.
					className="pointer-events-none relative inline-flex size-3.5 shrink-0 items-center justify-center text-muted-foreground transition-colors group-focus-within:pointer-events-auto group-focus-within:text-foreground group-hover:pointer-events-auto group-hover:text-foreground focus-visible:rounded-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
				>
					<Icon
						className="size-3.5 stroke-[1.5] opacity-100 transition-opacity group-focus-within:opacity-0 group-hover:opacity-0"
						aria-hidden="true"
					/>
					<XIcon
						className="absolute size-3.5 stroke-[1.5] opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100"
						aria-hidden="true"
					/>
				</button>
				<span>{label}</span>
			</TooltipTrigger>
			<TooltipContent side="top" align="start">
				<div>{description}</div>
				{isPlan && <div className="text-[11px] opacity-70">Shift + Tab to toggle</div>}
			</TooltipContent>
		</Tooltip>
	)
}

/**
 * Instant-scroll when session content finishes loading.
 *
 * The `<Conversation>` (StickToBottom) uses `initial="instant"` for the first
 * paint, but messages are fetched async — by the time they arrive and render,
 * the library treats the content growth as a *resize* and applies
 * `resize="smooth"`, causing a visible scroll animation from top → bottom.
 *
 * This component sits inside `<Conversation>` so it can access the
 * StickToBottom context. It watches for the loading→loaded transition
 * and forces an instant scroll-to-bottom.
 */
function ScrollOnLoad({ loading, sessionId }: { loading: boolean; sessionId: string }) {
	const { scrollToBottom } = useStickToBottomContext()
	const settingsOverlayOpen = useAtomValue(settingsOverlayOpenAtom)
	const prevLoadingRef = useRef(loading)
	const prevSessionRef = useRef(sessionId)

	useLayoutEffect(() => {
		const wasLoading = prevLoadingRef.current
		const sessionChanged = prevSessionRef.current !== sessionId
		prevLoadingRef.current = loading
		prevSessionRef.current = sessionId

		if (settingsOverlayOpen || getPendingRestoreScrollTop() != null) return

		// Instant scroll when: loading just finished, or session changed while not loading
		// (e.g. messages were already cached in the Jotai store)
		if ((wasLoading && !loading) || (sessionChanged && !loading)) {
			scrollToBottom("instant")
		}
	}, [loading, sessionId, scrollToBottom, settingsOverlayOpen])

	return null
}

/**
 * Tracks scroll position while the session is visible so it can be restored
 * after returning from Settings (StickToBottom may reset on layout changes).
 */
function ScrollPositionTracker({ sessionId }: { sessionId: string }) {
	const { scrollRef } = useStickToBottomContext()
	const setScrollTop = useSetAtom(sessionScrollTopFamily(sessionId))
	const settingsOverlayOpen = useAtomValue(settingsOverlayOpenAtom)

	useEffect(() => {
		const element = scrollRef.current
		if (!element) return

		const onScroll = () => {
			if (settingsOverlayOpen) return
			setScrollTop(element.scrollTop)
		}

		element.addEventListener("scroll", onScroll, { passive: true })
		return () => element.removeEventListener("scroll", onScroll)
	}, [scrollRef, sessionId, setScrollTop, settingsOverlayOpen])

	return null
}

/**
 * Handles scroll freeze/restore around the Settings overlay with StickToBottom context.
 */
function SettingsScrollGuard({ sessionId }: { sessionId: string }) {
	const { scrollRef, stopScroll } = useStickToBottomContext()
	const settingsOverlayOpen = useAtomValue(settingsOverlayOpenAtom)

	useLayoutEffect(() => {
		const wasOverlayOpen = trackSettingsOverlayOpen(settingsOverlayOpen)

		if (!wasOverlayOpen && settingsOverlayOpen) {
			const top = scrollRef.current?.scrollTop ?? getFrozenSessionScroll(sessionId) ?? null
			if (top != null) {
				freezeSessionScroll(sessionId, top)
			}
			stopScroll()
			return
		}

		if (!wasOverlayOpen || settingsOverlayOpen) return

		const frozen = getFrozenSessionScroll(sessionId)
		if (frozen == null) return

		setPendingRestoreScrollTop(frozen)
		markScrollRestored(frozen)

		const applyRestore = () => {
			if (!scrollRef.current) return
			scrollRef.current.scrollTop = frozen
			stopScroll()
		}
		applyRestore()
		requestAnimationFrame(() => {
			applyRestore()
			requestAnimationFrame(() => {
				applyRestore()
				setPendingRestoreScrollTop(null)
			})
		})
	}, [sessionId, settingsOverlayOpen, scrollRef, stopScroll])

	return null
}

interface ScrollHandle {
	scrollToBottom: (behavior?: "instant" | "smooth") => void
	/** Returns the current scrollHeight of the scroll container */
	getScrollHeight: () => number
	/** Returns the current scrollTop of the scroll container */
	getScrollTop: () => number
	/** Scrolls the container to a specific scrollTop value */
	scrollToPosition: (top: number, behavior?: ScrollBehavior) => void
}

/**
 * Bridge that exposes the StickToBottom `scrollToBottom` to the parent
 * via a ref so imperative callers (handleSend, question reply, etc.)
 * can force a scroll-to-bottom even when the user has scrolled away.
 * Also exposes scroll position helpers for the "jump to start" feature.
 */
function ScrollBridge({ scrollRef }: { scrollRef: React.RefObject<ScrollHandle | null> }) {
	const ctx = useStickToBottomContext()
	useImperativeHandle(
		scrollRef,
		() => ({
			scrollToBottom: (behavior?: "instant" | "smooth") => {
				ctx.scrollToBottom(behavior ?? "smooth")
			},
			getScrollHeight: () => {
				return ctx.scrollRef.current?.scrollHeight ?? 0
			},
			getScrollTop: () => {
				return ctx.scrollRef.current?.scrollTop ?? 0
			},
			scrollToPosition: (top: number, behavior: ScrollBehavior = "smooth") => {
				ctx.scrollRef.current?.scrollTo({ top, behavior })
			},
		}),
		[ctx],
	)
	return null
}

/** Prefetch older messages when the user scrolls toward the top of the thread. */
function LoadEarlierOnScroll({
	hasEarlierMessages,
	loadingEarlier,
	onLoadEarlier,
	scrollRef,
}: {
	hasEarlierMessages: boolean
	loadingEarlier: boolean
	onLoadEarlier?: () => void | Promise<void>
	scrollRef: RefObject<ScrollHandle | null>
}) {
	const { scrollRef: containerRef, stopScroll } = useStickToBottomContext()
	const sentinelRef = useRef<HTMLDivElement>(null)
	const loadingRef = useRef(loadingEarlier)
	loadingRef.current = loadingEarlier
	const hasEarlierRef = useRef(hasEarlierMessages)
	hasEarlierRef.current = hasEarlierMessages

	const loadWithScrollPreserve = useCallback(async () => {
		if (!onLoadEarlier || loadingRef.current || !hasEarlierRef.current) return
		stopScroll()
		const beforeHeight = scrollRef.current?.getScrollHeight() ?? 0
		const beforeTop = scrollRef.current?.getScrollTop() ?? 0
		await onLoadEarlier()
		requestAnimationFrame(() => {
			stopScroll()
			const afterHeight = scrollRef.current?.getScrollHeight() ?? 0
			scrollRef.current?.scrollToPosition(afterHeight - beforeHeight + beforeTop, "auto")
			requestAnimationFrame(() => stopScroll())
		})
	}, [onLoadEarlier, scrollRef, stopScroll])

	useEffect(() => {
		const root = containerRef.current
		const target = sentinelRef.current
		if (!root || !target || !hasEarlierMessages || !onLoadEarlier) return

		const observer = new IntersectionObserver(
			(entries) => {
				if (!entries[0]?.isIntersecting) return
				void loadWithScrollPreserve()
			},
			{ root, rootMargin: "160px 0px 0px 0px", threshold: 0 },
		)
		observer.observe(target)
		return () => observer.disconnect()
	}, [containerRef, hasEarlierMessages, loadWithScrollPreserve, onLoadEarlier])

	if (!hasEarlierMessages && !loadingEarlier) return null

	return (
		<div
			ref={sentinelRef}
			className="flex min-h-8 justify-center py-2"
			aria-busy={loadingEarlier}
			aria-live="polite"
		>
			{loadingEarlier ? (
				<Loader2Icon className="size-4 animate-spin text-muted-foreground/70" />
			) : (
				<span className="sr-only">Scroll up to load earlier messages</span>
			)}
		</div>
	)
}

const TURN_ESTIMATE_MIN = 120
const TURN_ESTIMATE_MAX = 12_000

function estimateTurnSize(turn: ChatTurn): number {
	let partCount = turn.userMessage.parts.length
	for (const message of turn.assistantMessages) {
		partCount += message.parts.length
	}
	const assistantTextLength = turn.assistantMessages.reduce((total, message) => {
		return (
			total +
			message.parts.reduce((messageTotal, part) => {
				return messageTotal + (part.type === "text" || part.type === "reasoning" ? part.text.length : 0)
			}, 0)
		)
	}, 0)
	const toolCount = turn.assistantMessages.reduce((total, message) => {
		return total + message.parts.filter((part) => part.type === "tool").length
	}, 0)
	const reasoningCount = turn.assistantMessages.reduce((total, message) => {
		return total + message.parts.filter((part) => part.type === "reasoning").length
	}, 0)
	const estimated =
		180 +
		turn.assistantMessages.length * 72 +
		partCount * 36 +
		toolCount * 96 +
		reasoningCount * 48 +
		assistantTextLength / 10
	return Math.max(TURN_ESTIMATE_MIN, Math.min(TURN_ESTIMATE_MAX, estimated))
}

function turnListRevision(turns: ChatTurn[]): string {
	return turns
		.map((turn) => {
			let partCount = turn.userMessage.parts.length
			let textLength = 0
			for (const message of turn.assistantMessages) {
				partCount += message.parts.length
				for (const part of message.parts) {
					if (part.type === "text" || part.type === "reasoning") {
						textLength += part.text.length
					}
				}
			}
			return `${turn.id}:${partCount}:${textLength}`
		})
		.join("|")
}

interface VirtualizedTurnListProps {
	turns: ChatTurn[]
	renderTurn: (turn: ChatTurn, index: number) => ReactNode
}

function VirtualizedTurnList({ turns, renderTurn }: VirtualizedTurnListProps) {
	const { scrollRef } = useStickToBottomContext()
	const turnsRevision = useMemo(() => turnListRevision(turns), [turns])
	const virtualizer = useVirtualizer({
		count: turns.length,
		getScrollElement: () => scrollRef.current,
		getItemKey: (index) => turns[index]?.id ?? index,
		estimateSize: (index) => estimateTurnSize(turns[index]),
		overscan: 8,
	})

	useLayoutEffect(() => {
		virtualizer.measure()
	}, [turnsRevision, virtualizer])

	return (
		<div
			style={{
				height: `${virtualizer.getTotalSize()}px`,
				position: "relative",
				width: "100%",
			}}
		>
			{virtualizer.getVirtualItems().map((virtualRow) => {
				const turn = turns[virtualRow.index]
				return (
					<div
						key={virtualRow.key}
						data-index={virtualRow.index}
						ref={virtualizer.measureElement}
						style={{
							position: "absolute",
							top: 0,
							left: 0,
							width: "100%",
							transform: `translateY(${virtualRow.start}px)`,
						}}
					>
						<div style={{ paddingBottom: VIRTUAL_TURN_GAP }}>
							{renderTurn(turn, virtualRow.index)}
						</div>
					</div>
				)
			})}
		</div>
	)
}

/**
 * Floating pill button that appears when the agent finishes working.
 * Scrolls to the beginning of the last assistant response so the user
 * can read it from the top. Dismisses on click or after 8 seconds.
 *
 * Captures the scroll container's scrollHeight when the agent starts
 * working (idle-to-working transition). This position corresponds to
 * "where the new response began" regardless of whether the agent
 * started from a fresh message, a question answer, or a permission grant.
 *
 * Must be rendered inside `<Conversation>` to position correctly.
 */
function ScrollToResponseStart({
	isWorking,
	scrollRef,
}: {
	isWorking: boolean
	scrollRef: React.RefObject<ScrollHandle | null>
}) {
	const [visible, setVisible] = useState(false)
	const prevWorkingRef = useRef(isWorking)
	// Saved scrollHeight at the moment the agent started working.
	// This is the Y position where the new response content begins.
	const savedScrollTopRef = useRef(0)

	useEffect(() => {
		const wasWorking = prevWorkingRef.current
		prevWorkingRef.current = isWorking

		if (!wasWorking && isWorking) {
			// Agent just started working -- snapshot where the response will begin.
			// scrollHeight is the total content height; subtracting a small offset
			// so the scroll lands slightly above the first new content.
			const handle = scrollRef.current
			if (handle) {
				savedScrollTopRef.current = Math.max(0, handle.getScrollHeight() - 80)
			}
		}

		if (wasWorking && !isWorking) {
			// Agent finished -- show the pill
			setVisible(true)
		}

		if (isWorking) {
			setVisible(false)
		}
	}, [isWorking, scrollRef])

	// Auto-dismiss after 8 seconds
	useEffect(() => {
		if (!visible) return
		const timer = setTimeout(() => setVisible(false), 8000)
		return () => clearTimeout(timer)
	}, [visible])

	const handleClick = useCallback(() => {
		scrollRef.current?.scrollToPosition(savedScrollTopRef.current)
		setVisible(false)
	}, [scrollRef])

	if (!visible) return null

	return (
		<button
			type="button"
			onClick={handleClick}
			className="absolute bottom-[calc(var(--chat-composer-inset)+3.5rem)] left-1/2 z-20 flex -translate-x-1/2 items-center gap-1.5 rounded-full border border-border bg-background px-3 py-1.5 text-xs text-muted-foreground shadow-md transition-colors hover:bg-muted hover:text-foreground"
		>
			<ArrowUpToLineIcon className="size-3" />
			<span>Jump to start of response</span>
		</button>
	)
}

/**
 * Bridge component that syncs the PromptInputProvider's text state
 * to the persisted draft store (debounced). Must be rendered inside
 * both a <PromptInputProvider> and receive draft actions for the session.
 */
function DraftSync({ setDraft }: { setDraft: (text: string) => void }) {
	const controller = usePromptInputController()
	const value = controller.textInput.value
	const isFirstRender = useRef(true)

	useEffect(() => {
		// Skip the initial render — the provider was just hydrated from the draft
		if (isFirstRender.current) {
			isFirstRender.current = false
			return
		}
		setDraft(value)
	}, [value, setDraft])

	return null
}

/**
 * Bridge that exposes the PromptInputProvider's text controller to the parent
 * via a ref, so handleSlashCommand can read/write the input text.
 */
function SlashCommandBridge({
	controllerRef,
}: {
	controllerRef: React.RefObject<{ setText: (text: string) => void; getText: () => string } | null>
}) {
	const controller = usePromptInputController()

	useEffect(() => {
		if (controllerRef && "current" in controllerRef) {
			;(controllerRef as React.MutableRefObject<typeof controllerRef.current>).current = {
				setText: (text: string) => controller.textInput.setInput(text),
				getText: () => controller.textInput.value,
			}
		}
		return () => {
			if (controllerRef && "current" in controllerRef) {
				;(controllerRef as React.MutableRefObject<typeof controllerRef.current>).current = null
			}
		}
	}, [controller, controllerRef])

	return null
}

/**
 * Bridge that detects `/` and `@` triggers from the text input
 * and syncs popover state. Must be rendered inside PromptInputProvider.
 *
 * Uses DOM queries to find the textarea for cursor position (since
 * PromptInputTextarea doesn't support ref forwarding).
 */
function TriggerDetector({
	onSlashChange,
	onMentionChange,
}: {
	onSlashChange: (open: boolean, query: string) => void
	onMentionChange: (open: boolean, query: string) => void
}) {
	const controller = usePromptInputController()
	const inputText = controller.textInput.value

	useEffect(() => {
		// Find textarea via DOM query (PromptInputTextarea doesn't forward refs)
		const textarea = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
		const cursorPos = textarea?.selectionStart ?? inputText.length
		const textBeforeCursor = inputText.slice(0, cursorPos)

		// Slash command: entire input starts with / and no space yet
		const slashMatch = inputText.match(/^\/(\S*)$/)
		if (slashMatch) {
			onSlashChange(true, slashMatch[1])
			onMentionChange(false, "")
			return
		}

		// @mention: @ followed by non-whitespace before cursor
		const atMatch = textBeforeCursor.match(/@(\S*)$/)
		if (atMatch) {
			onMentionChange(true, atMatch[1])
			onSlashChange(false, "")
			return
		}

		// No trigger
		onSlashChange(false, "")
		onMentionChange(false, "")
	}, [inputText, onSlashChange, onMentionChange])

	return null
}

/**
 * Bridge that reconciles mentions with the current text.
 * When the user manually deletes an `@mention` marker from the text,
 * this removes the corresponding entry from the mentions list.
 * Must be rendered inside PromptInputProvider.
 */
function MentionReconciler({
	mentions,
	onReconcile,
}: {
	mentions: PromptMention[]
	onReconcile: (updated: PromptMention[]) => void
}) {
	const controller = usePromptInputController()
	const inputText = controller.textInput.value

	useEffect(() => {
		if (mentions.length === 0) return
		const reconciled = reconcileMentions(mentions, inputText)
		if (reconciled.length !== mentions.length) {
			onReconcile(reconciled)
		}
	}, [inputText, mentions, onReconcile])

	return null
}

interface ChatViewProps {
	turns: ChatTurn[]
	loading: boolean
	/** Whether earlier messages are currently being loaded */
	loadingEarlier: boolean
	/** Whether there are earlier messages that can be loaded */
	hasEarlierMessages: boolean
	/** Callback to load earlier messages */
	onLoadEarlier?: () => void | Promise<void>
	agent: Agent
	isConnected: boolean
	onSendMessage?: (
		agent: Agent,
		message: string,
		options?: { model?: ModelRef; agentName?: string; variant?: string; files?: FileAttachment[]; collaborationMode?: string },
	) => Promise<void>
	/** Callback to stop/abort the running session */
	onStop?: (agent: Agent) => Promise<void>
	/** Provider data for model selector */
	providers?: ProvidersData | null
	/** Config data (default model, default agent) */
	config?: ConfigData | null
	/** VCS data, currently consumed by non-composer surfaces only. */
	vcs?: VcsData | null
	/** Available Devo agents */
	devoAgents?: SdkAgent[]
	/** Permission handlers */
	onApprove?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		response?: PermissionResponse,
	) => Promise<void>
	onDeny?: (agent: Agent, permissionSessionId: string, permissionId: string) => Promise<void>
	/** Question handlers */
	onReplyQuestion?: (agent: Agent, requestId: string, answers: QuestionAnswer[]) => Promise<void>
	onRejectQuestion?: (agent: Agent, requestId: string) => Promise<void>
	/** Undo/redo */
	canUndo?: boolean
	canRedo?: boolean
	onUndo?: () => Promise<string | undefined>
	onRedo?: () => Promise<void>
	isReverted?: boolean
	/** Revert to a specific message (for per-turn undo) */
	onRevertToMessage?: (messageId: string) => Promise<void>
	/** Fork from a turn boundary (messageId of the next turn's user message, or undefined for full fork) */
	onForkFromTurn?: (messageId?: string) => Promise<void>
	/** Delete a specific part from a message (for error recovery) */
	onDeletePart?: (sessionId: string, messageId: string, partId: string) => Promise<void>
	/** Whether the review panel is open (removes max-w constraint) */
	reviewPanelOpen?: boolean
}

/**
 * Main chat view component.
 * Renders the full conversation as turns with auto-scroll,
 * plus a card-style input with agent/model/variant toolbar and status bar.
 *
 * The input section (toolbar, popovers, mentions, model/agent/variant state)
 * is extracted into `ChatInputSection` so that state changes in the input area
 * don't cause re-renders of the conversation turn list.
 */
export function ChatView({
	turns,
	loading,
	loadingEarlier,
	hasEarlierMessages,
	onLoadEarlier,
	agent,
	isConnected,
	onSendMessage,
	onStop,
	providers,
	config,
	devoAgents,
	onApprove,
	onDeny,
	onReplyQuestion,
	onRejectQuestion,
	canUndo,
	canRedo,
	onUndo,
	onRedo,
	isReverted,
	onRevertToMessage,
	onForkFromTurn,
	onDeletePart,
	reviewPanelOpen,
}: ChatViewProps) {
	const isWorking = agent.status === "running"
	const settingsOverlayOpen = useAtomValue(settingsOverlayOpenAtom)

	const conversationTargetScrollTop = useCallback(
		(defaultTarget: number) => {
			const restored = getRestoredScrollTop()
			if (restored != null) return restored
			const pendingRestore = getPendingRestoreScrollTop()
			if (pendingRestore != null) return pendingRestore
			if (settingsOverlayOpen) {
				const frozen = getFrozenSessionScroll(agent.sessionId)
				if (frozen != null) return frozen
			}
			return defaultTarget
		},
		[agent.sessionId, settingsOverlayOpen],
	)

	// Ref to imperatively scroll the conversation to bottom from outside the
	// <Conversation> tree (e.g. after sending a message or answering a question).
	const scrollRef = useRef<ScrollHandle | null>(null)
	const composerRef = useRef<HTMLDivElement | null>(null)
	const [composerInset, setComposerInset] = useState(0)

	// Session-level error and setup phase from the session atom
	const sessionEntry = useAtomValue(sessionFamily(agent.sessionId))
	const sessionError = sessionEntry?.error
	const setupPhase = sessionEntry?.setupPhase
	const compactionStatus = useAtomValue(compactionStatusFamily(agent.sessionId))

	useLayoutEffect(() => {
		if (setupPhase) {
			setComposerInset(0)
			return
		}

		const composer = composerRef.current
		if (!composer) return

		const updateComposerInset = () => {
			const nextInset = Math.ceil(composer.getBoundingClientRect().height)
			setComposerInset((currentInset) =>
				currentInset === nextInset ? currentInset : nextInset,
			)
		}

		updateComposerInset()

		if (typeof ResizeObserver === "undefined") {
			if (typeof window !== "undefined") {
				window.addEventListener("resize", updateComposerInset)
			}
			return () => {
				if (typeof window !== "undefined") {
					window.removeEventListener("resize", updateComposerInset)
				}
			}
		}

		const resizeObserver = new ResizeObserver(updateComposerInset)
		resizeObserver.observe(composer)
		if (typeof window !== "undefined") {
			window.addEventListener("resize", updateComposerInset)
		}

		return () => {
			resizeObserver.disconnect()
			if (typeof window !== "undefined") {
				window.removeEventListener("resize", updateComposerInset)
			}
		}
	}, [setupPhase])
	const effectivePermission = useAtomValue(effectivePermissionFamily(agent.sessionId))
	const removePermission = useSetAtom(removePermissionAtom)

	// Format the session-level error for display. Only shown when the last
	// turn doesn't already carry an assistant-level error (the server emits
	// both session.error and message.updated for the same failure, so showing
	// both would duplicate the message).
	const sessionErrorText = useMemo(() => {
		if (!sessionError) return undefined
		if ("message" in sessionError.data && sessionError.data.message) {
			return String(sessionError.data.message)
		}
		return `${sessionError.name}: ${JSON.stringify(sessionError.data)}`
	}, [sessionError])

	const lastTurnHasError = useMemo(() => {
		const lastTurn = turns.at(-1)
		if (!lastTurn) return false
		return lastTurn.assistantMessages.some(
			(m) => m.info.role === "assistant" && m.info.error != null,
		)
	}, [turns])

	const showSessionError = !!sessionErrorText && !lastTurnHasError

	// Stable callbacks for question/permission handlers — agent is stable
	// per render, but wrapping in useCallback avoids creating new inline
	// closures inside the JSX .map() that would defeat memo() on children.
	const handleApprovePermission = useCallback(
		async (
			a: Agent,
			permissionSessionId: string,
			permissionId: string,
			response?: PermissionResponse,
		) => {
			await onApprove?.(a, permissionSessionId, permissionId, response)
			removePermission({ sessionId: permissionSessionId, permissionId })
			// Permission card disappears after approval — scroll to keep content visible.
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onApprove, removePermission],
	)

	const handleDenyPermission = useCallback(
		async (a: Agent, permissionSessionId: string, permissionId: string) => {
			await onDeny?.(a, permissionSessionId, permissionId)
			removePermission({ sessionId: permissionSessionId, permissionId })
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onDeny, removePermission],
	)

	// Keyboard shortcuts for undo/redo
	useEffect(() => {
		const handleKeyDown = (e: KeyboardEvent) => {
			// Don't intercept Cmd/Ctrl+Z in any text input — let the browser
			// handle native undo/redo. Session undo/redo is still available via
			// /undo, /redo slash commands and the command palette.
			const target = e.target as HTMLElement
			if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return

			// Cmd+Z / Ctrl+Z — Undo
			if ((e.metaKey || e.ctrlKey) && e.key === "z" && !e.shiftKey) {
				if (canUndo && onUndo) {
					e.preventDefault()
					onUndo()
				}
				return
			}

			// Cmd+Shift+Z / Ctrl+Shift+Z — Redo
			if ((e.metaKey || e.ctrlKey) && e.key === "z" && e.shiftKey) {
				if (canRedo && onRedo) {
					e.preventDefault()
					onRedo()
				}
				return
			}
		}

		document.addEventListener("keydown", handleKeyDown)
		return () => document.removeEventListener("keydown", handleKeyDown)
	}, [canUndo, canRedo, onUndo, onRedo])

	// Width constraint class: remove max-w when review panel is open
	const contentWidthClass = reviewPanelOpen
		? "mx-auto w-full min-w-0"
		: "mx-auto w-full min-w-0 max-w-3xl"

	const retryStatus = sessionEntry?.retryStatus

	const renderTurn = useCallback(
		(turn: ChatTurn, index: number) => {
			const isLastTurn = index === turns.length - 1
			// Prefer protocol turn id when present. Fall back to the active last turn
			// because the server already scopes retry status to the session's active turn.
			const activeRetryStatus: ProviderRetryStatus | undefined =
				!retryStatus || !isLastTurn
					? undefined
					: turn.turnId
						? retryStatus.turnId === turn.turnId
							? retryStatus
							: undefined
						: isWorking
							? retryStatus
							: undefined
			return (
			<ChatTurnComponent
				key={turn.id}
				turn={turn}
				isLast={isLastTurn}
				isWorking={isWorking}
				agent={agent}
				pendingPermission={index === turns.length - 1 ? effectivePermission : undefined}
				isConnected={isConnected}
				compactionStatus={compactionStatus}
				retryStatus={activeRetryStatus}
				onApprovePermission={handleApprovePermission}
				onDenyPermission={handleDenyPermission}
				onRevertToMessage={onRevertToMessage}
				onForkFromTurn={
					onForkFromTurn
						? () => {
								const nextTurn = turns[index + 1]
								return onForkFromTurn(nextTurn?.userMessage.info.id)
							}
						: undefined
				}
				onDeletePart={onDeletePart}
				onImplementPlan={
					onSendMessage
						? () => {
								appStore.set(collaborationModeFamily(agent.sessionId), "build")
								void onSendMessage(agent, "Implement Plan", { collaborationMode: "build" })
							}
						: undefined
				}
				onRevisePlan={() => {
					appStore.set(collaborationModeFamily(agent.sessionId), "plan")
				}}
			/>
			)
		},
		[
			agent,
			effectivePermission,
			compactionStatus,
			handleApprovePermission,
			handleDenyPermission,
			isConnected,
			isWorking,
			onDeletePart,
			onForkFromTurn,
			onRevertToMessage,
			onSendMessage,
			retryStatus,
			turns,
		],
	)

	return (
		<div
			className="relative flex h-full min-w-0 flex-col overflow-hidden"
			style={
				{
					"--chat-composer-inset": setupPhase ? "0px" : `${composerInset}px`,
				} as CSSProperties
			}
		>
			{/* Chat messages -- constrained width for readability */}
			<div className="relative min-h-0 min-w-0 flex-1">
				<Conversation
					key={agent.sessionId}
					className="h-full"
					targetScrollTop={conversationTargetScrollTop}
				>
					<ScrollOnLoad loading={loading} sessionId={agent.sessionId} />
					<SettingsScrollGuard sessionId={agent.sessionId} />
					<ScrollPositionTracker sessionId={agent.sessionId} />
					<ScrollBridge scrollRef={scrollRef} />
					<ConversationContent
						scrollClassName="scrollbar-comfort"
						className="gap-12 px-6 pt-4 pb-[calc(var(--chat-composer-inset)+1rem)] sm:px-10 sm:pt-8 sm:pb-[calc(var(--chat-composer-inset)+1.5rem)] lg:px-12"
					>
						<div className={cn(contentWidthClass, "space-y-12")}>
							<LoadEarlierOnScroll
								hasEarlierMessages={hasEarlierMessages}
								loadingEarlier={loadingEarlier}
								onLoadEarlier={onLoadEarlier}
								scrollRef={scrollRef}
							/>

							{loading ? (
								<div className="flex items-center justify-center py-8">
									<Loader2Icon className="size-5 animate-spin text-muted-foreground" />
									<span className="ml-2 text-sm text-muted-foreground">Loading chat...</span>
								</div>
							) : turns.length > 0 ? (
								turns.length > VIRTUALIZE_TURN_THRESHOLD ? (
									<VirtualizedTurnList turns={turns} renderTurn={renderTurn} />
								) : (
									turns.map(renderTurn)
								)
							) : setupPhase ? (
								<WorktreeSetupProgress phase={setupPhase} />
							) : (
								<div className="flex items-center justify-center py-8">
									<p className="text-sm text-muted-foreground">No messages yet</p>
								</div>
							)}

							{/* Session-level error from session.error events */}
							{showSessionError && sessionErrorText && (
								<div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
									{sessionErrorText}
								</div>
							)}
						</div>
					</ConversationContent>
					<ScrollToResponseStart isWorking={isWorking} scrollRef={scrollRef} />
					<ConversationScrollButton className="!bottom-[calc(var(--chat-composer-inset)+1rem)]" />
				</Conversation>

				{/* Top fade */}
				<div
					data-slot="scroll-fade"
					aria-hidden="true"
					className="pointer-events-none absolute inset-x-0 top-0 z-10 h-6 bg-gradient-to-b from-background/30 to-transparent"
				/>
				{/* Bottom fade */}
				<div
					data-slot="scroll-fade"
					aria-hidden="true"
					className="pointer-events-none absolute inset-x-0 bottom-[var(--chat-composer-inset)] z-10 h-6 bg-gradient-to-t from-background/30 to-transparent"
				/>
			</div>

			{/* Bottom input section — hidden during worktree setup since the stub session
			   cannot accept prompts yet. Extracted into its own component so toolbar,
			   popover, mention, and model-selection state changes don't re-render the
			   conversation turn list above. */}
			{!setupPhase && (
				<div
					ref={composerRef}
					className="pointer-events-none absolute bottom-0 left-0 right-3.5 z-30 overflow-visible pt-3"
				>
					<ChatInputSection
						agent={agent}
						turns={turns}
						isConnected={isConnected}
						isWorking={isWorking}
						onSendMessage={onSendMessage}
						onStop={onStop}
						providers={providers}
						config={config}
						devoAgents={devoAgents}
						onApprove={handleApprovePermission}
						onDeny={handleDenyPermission}
						onReplyQuestion={onReplyQuestion}
						onRejectQuestion={onRejectQuestion}
						canRedo={canRedo}
						onRedo={onRedo}
						isReverted={isReverted}
						scrollRef={scrollRef}
						reviewPanelOpen={reviewPanelOpen}
					/>
				</div>
			)}
		</div>
	)
}

// ============================================================
// ChatInputSection — owns all input/toolbar/popover/mention state
// ============================================================

interface ChatInputSectionProps {
	agent: Agent
	turns: ChatTurn[]
	isConnected: boolean
	isWorking: boolean
	onSendMessage?: ChatViewProps["onSendMessage"]
	onStop?: ChatViewProps["onStop"]
	providers?: ProvidersData | null
	config?: ConfigData | null
	devoAgents?: SdkAgent[]
	onApprove?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		response?: PermissionResponse,
	) => Promise<void>
	onDeny?: (agent: Agent, permissionSessionId: string, permissionId: string) => Promise<void>
	onReplyQuestion?: ChatViewProps["onReplyQuestion"]
	onRejectQuestion?: ChatViewProps["onRejectQuestion"]
	canRedo?: boolean
	onRedo?: () => Promise<void>
	isReverted?: boolean
	scrollRef: React.RefObject<ScrollHandle | null>
	reviewPanelOpen?: boolean
}

function ChatInputSection({
	agent,
	turns,
	isConnected,
	isWorking,
	onSendMessage,
	onStop,
	providers,
	config,
	devoAgents,
	onApprove,
	onDeny,
	onReplyQuestion,
	onRejectQuestion,
	canRedo,
	onRedo,
	isReverted,
	scrollRef,
	reviewPanelOpen,
}: ChatInputSectionProps) {
	const [sending, setSending] = useState(false)
	const [activeTrigger, setActiveTrigger] = useState<ComposerTrigger | null>(null)
	const [activeGoal, setActiveGoal] = useState<ComposerGoal | null>(null)
	const [goalAction, setGoalAction] = useState<ComposerGoalAction | null>(null)
	const [skillPickerOpen, setSkillPickerOpen] = useState(false)
	const [collaborationMode, setCollaborationMode] = useAtom(
		collaborationModeFamily(agent.sessionId),
	)

	// User requirement: the /goal footer chip is only an input trigger;
	// the composer-adjacent status row reflects the real session goal state.
	useEffect(() => {
		setActiveTrigger(null)
		setActiveGoal(null)
		setGoalAction(null)
	}, [agent.sessionId])

	const loadGoalStatus = useCallback(async (): Promise<ComposerGoal | null> => {
		if (!agent.directory) return null
		const client = getProjectClient(agent.directory)
		if (!client?.goal?.status) return null
		const result = await client.goal.status({ sessionID: agent.sessionId })
		return normalizeComposerGoal(result.data)
	}, [agent.directory, agent.sessionId])

	const refreshGoalStatus = useCallback(async () => {
		try {
			setActiveGoal(await loadGoalStatus())
		} catch (err) {
			log.error("goal.status failed", { sessionId: agent.sessionId }, err)
		}
	}, [agent.sessionId, loadGoalStatus])

	useEffect(() => {
		let disposed = false
		const load = async () => {
			try {
				const nextGoal = await loadGoalStatus()
				if (!disposed) setActiveGoal(nextGoal)
			} catch (err) {
				if (!disposed) {
					setActiveGoal(null)
					log.error("goal.status failed", { sessionId: agent.sessionId }, err)
				}
			}
		}
		void load()
		const interval = setInterval(load, 15_000)
		return () => {
			disposed = true
			clearInterval(interval)
		}
	}, [agent.sessionId, loadGoalStatus])

	useEffect(() => {
		if (!isWorking) void refreshGoalStatus()
	}, [isWorking, refreshGoalStatus])

	// Tree-scoped interactive requests — bubbles up from sub-agent sessions.
	// These replace the direct `agent.permissions` / `agent.questions` arrays
	// so the parent session's UI can respond on behalf of any descendant.
	const effectivePermission = useAtomValue(effectivePermissionFamily(agent.sessionId))
	const effectiveQuestion = useAtomValue(effectiveQuestionFamily(agent.sessionId))
	const removePermission = useSetAtom(removePermissionAtom)

	// Diff comments integration
	const diffComments = useAtomValue(diffCommentsFamily(agent.sessionId))
	const setDiffComments = useSetAtom(diffCommentsFamily(agent.sessionId))

	// Elapsed-time split for the current turn — used for the live timer on the submit button.
	const currentTurnWorkSplit = useMemo(() => {
		if (!isWorking || turns.length === 0) return null
		const lastTurn = turns[turns.length - 1]
		if (lastTurn.assistantMessages.length === 0) return null
		return computeTurnWorkTimeSplit(lastTurn)
	}, [isWorking, turns])

	// Mention tracking — files and agents referenced via @
	const [mentions, setMentions] = useState<PromptMention[]>([])

	// Reset mentions when session changes
	// biome-ignore lint/correctness/useExhaustiveDependencies: intentional — clear on session switch
	useEffect(() => {
		setMentions([])
	}, [agent.sessionId])

	// Stable callbacks for question/permission handlers
	const handleReplyQuestion = useCallback(
		async (requestId: string, answers: QuestionAnswer[]) => {
			await onReplyQuestion?.(agent, requestId, answers)
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onReplyQuestion, agent, scrollRef],
	)

	const handleRejectQuestion = useCallback(
		async (requestId: string) => {
			await onRejectQuestion?.(agent, requestId)
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onRejectQuestion, agent, scrollRef],
	)

	const handleApprovePermission = useCallback(
		async (
			a: Agent,
			permissionSessionId: string,
			permissionId: string,
			response?: PermissionResponse,
		) => {
			await onApprove?.(a, permissionSessionId, permissionId, response)
			removePermission({ sessionId: permissionSessionId, permissionId })
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onApprove, removePermission, scrollRef],
	)

	const handleDenyPermission = useCallback(
		async (a: Agent, permissionSessionId: string, permissionId: string) => {
			await onDeny?.(a, permissionSessionId, permissionId)
			removePermission({ sessionId: permissionSessionId, permissionId })
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onDeny, removePermission, scrollRef],
	)

	// Draft persistence
	const draft = useDraftSnapshot(agent.sessionId)
	const { setDraft, clearDraft } = useDraftActions(agent.sessionId)

	// Escape-to-abort: double-press within 3s
	const [, setInterruptCount] = useState(0)
	const interruptTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

	// Toolbar state
	const [selectedModel, setSelectedModel] = useState<ModelRef | null>(null)
	const [selectedAgent, setSelectedAgent] = useState<string | null>(null)
	const [selectedVariant, setSelectedVariant] = useState<string | undefined>(undefined)

	// Initialize model, variant, and agent from the session's last user message.
	const sessionMessages = useAtomValue(messagesFamily(agent.sessionId))
	const projectModels = useAtomValue(projectModelsAtom)
	const initializedForSessionRef = useRef<string | null>(null)
	const resetForSessionRef = useRef<string | null>(null)
	useEffect(() => {
		if (resetForSessionRef.current !== agent.sessionId) {
			resetForSessionRef.current = agent.sessionId
			initializedForSessionRef.current = null
			const stored = agent.directory ? projectModels[agent.directory] : undefined
			if (stored?.providerID && stored?.modelID) {
				setSelectedModel(stored)
				setSelectedVariant(stored.variant)
			} else {
				setSelectedModel(null)
				setSelectedVariant(undefined)
			}
			setSelectedAgent(stored?.agent || null)
		}

		if (initializedForSessionRef.current === agent.sessionId) return
		if (!sessionMessages || sessionMessages.length === 0) return
		initializedForSessionRef.current = agent.sessionId

		let foundModel = false
		let foundAgent = false
		for (let i = sessionMessages.length - 1; i >= 0; i--) {
			const msg = sessionMessages[i]
			if (msg.role !== "user") continue
			const dynamic = msg as Record<string, unknown>

			if (!foundModel && "model" in msg && msg.model) {
				const model = msg.model as { providerID: string; modelID: string }
				if (model.providerID && model.modelID) {
					setSelectedModel(model)
					foundModel = true
					const variant = dynamic.variant as string | undefined
					if (variant) {
						setSelectedVariant(variant)
					} else {
						setSelectedVariant(undefined)
					}
				}
			}

			if (
				!foundAgent &&
				dynamic.agent &&
				typeof dynamic.agent === "string" &&
				dynamic.agent.length > 0
			) {
				setSelectedAgent(dynamic.agent)
				foundAgent = true
			}

			if (foundModel && foundAgent) break
		}
	}, [sessionMessages, agent.sessionId, agent.directory, projectModels])

	const { addRecent: addRecentModel } = useModelState()

	const activeDevoAgent = useMemo(() => {
		const agentName = selectedAgent ?? config?.defaultAgent
		return devoAgents?.find((a) => a.name === agentName) ?? null
	}, [selectedAgent, config?.defaultAgent, devoAgents])

	const effectiveModel = useMemo(
		() =>
			resolveEffectiveModel(
				selectedModel,
				activeDevoAgent,
				config?.model,
				providers?.defaults ?? {},
				providers?.providers ?? [],
			),
		[selectedModel, activeDevoAgent, config?.model, providers],
	)

	useEffect(() => {
		if (!selectedVariant || !effectiveModel || !providers) return
		const available = getModelVariants(
			effectiveModel.providerID,
			effectiveModel.modelID,
			providers.providers,
		)
		if (!available.includes(selectedVariant)) {
			setSelectedVariant(undefined)
		}
	}, [selectedVariant, effectiveModel, providers])

	const modelCapabilities = useMemo(
		() => getModelInputCapabilities(effectiveModel, providers?.providers ?? []),
		[effectiveModel, providers],
	)

	const handleModelSelect = useCallback(
		(model: ModelRef | null) => {
			setSelectedModel(model)
			setSelectedVariant(undefined)
			if (!model) return
			addRecentModel(model)
			if (!agent.directory) return
			void persistRuntimeModelSelection(agent.directory, model).catch((err) => {
				console.error("Failed to persist model selection:", err)
			})
		},
		[addRecentModel, agent.directory],
	)

	const handleVariantSelect = useCallback(
		(variant: string | undefined) => {
			setSelectedVariant(variant)
			if (!variant || !agent.directory) return
			void persistRuntimeModelConfigOption(agent.directory, "thought_level", variant).catch((err) => {
				console.error("Failed to persist reasoning effort selection:", err)
			})
		},
		[agent.directory],
	)

	const slashCommandRef = useRef<{
		setText: (text: string) => void
		getText: () => string
	} | null>(null)

	const focusComposer = useCallback(() => {
		requestAnimationFrame(() => {
			const textarea = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
			textarea?.focus()
		})
	}, [])

	const handleEditGoal = useCallback(() => {
		if (!activeGoal) return
		setActiveTrigger("goal")
		slashCommandRef.current?.setText(activeGoal.objective)
		focusComposer()
	}, [activeGoal, focusComposer])

	const handlePauseGoal = useCallback(async () => {
		if (!agent.directory || goalAction !== null) return
		const client = getProjectClient(agent.directory)
		if (!client?.goal?.pause) return
		setGoalAction("pause")
		try {
			const result = await client.goal.pause({ sessionID: agent.sessionId })
			setActiveGoal(normalizeComposerGoal(result.data))
		} catch (err) {
			log.error("goal.pause failed", { sessionId: agent.sessionId }, err)
		} finally {
			setGoalAction(null)
		}
	}, [agent.directory, agent.sessionId, goalAction])

	const handleResumeGoal = useCallback(async () => {
		if (!agent.directory || goalAction !== null) return
		const client = getProjectClient(agent.directory)
		if (!client?.goal?.resume) return
		setGoalAction("resume")
		try {
			const result = await client.goal.resume({ sessionID: agent.sessionId })
			setActiveGoal(normalizeComposerGoal(result.data))
		} catch (err) {
			log.error("goal.resume failed", { sessionId: agent.sessionId }, err)
		} finally {
			setGoalAction(null)
		}
	}, [agent.directory, agent.sessionId, goalAction])

	const handleClearGoal = useCallback(async () => {
		if (!agent.directory || goalAction !== null) return
		const client = getProjectClient(agent.directory)
		if (!client?.goal?.clear) return
		setGoalAction("clear")
		try {
			await client.goal.clear({ sessionID: agent.sessionId })
			setActiveGoal(null)
		} catch (err) {
			log.error("goal.clear failed", { sessionId: agent.sessionId }, err)
		} finally {
			setGoalAction(null)
		}
	}, [agent.directory, agent.sessionId, goalAction])

	const handleSlashCommand = useCallback(
		async (text: string): Promise<boolean> => {
			const trimmed = text.trim()
			if (!trimmed.startsWith("/")) return false

			const spaceIndex = trimmed.indexOf(" ")
			const cmdName = spaceIndex === -1 ? trimmed.slice(1) : trimmed.slice(1, spaceIndex)

			// Product requirement: Desktop slash commands are limited to first-party
			// entries. Compact executes immediately; Goal/Plan become footer trigger
			// chips; Research stays as slash text so Native can run it after a question.
			switch (cmdName.toLowerCase()) {
				case "compact":
					if (agent.directory && effectiveModel) {
						const client = getProjectClient(agent.directory)
						if (client) {
							try {
								await client.session.summarize({
									sessionID: agent.sessionId,
									providerID: effectiveModel.providerID,
									modelID: effectiveModel.modelID,
								})
							} catch (err) {
								log.error("session.summarize failed", { sessionId: agent.sessionId }, err)
							}
						}
					}
					return true
				case "goal":
					setActiveTrigger("goal")
					return true
				case "plan":
					setActiveTrigger("plan")
					setCollaborationMode("plan")
					return true
				case "skills":
					setSkillPickerOpen(true)
					return true
				case "research":
					return false
				default:
					return false
			}
		},
		[agent.directory, agent.sessionId, effectiveModel, setCollaborationMode],
	)

	const submitTriggeredPrompt = useCallback(
		async (trigger: ComposerTrigger, text: string, files?: FileAttachment[]) => {
			if (!agent.directory) throw new Error("No project directory for slash trigger")
			const client = getProjectClient(agent.directory)
			if (!client) throw new Error("Not connected to Devo server")
			const parts: Array<
				{ type: "text"; text: string } | {
					type: "file"
					mime: string
					filename?: string
					url: string
				}
			> = [{ type: "text", text: `/${trigger} ${text.trim()}` }]
			for (const file of files ?? []) {
				parts.push({
					type: "file",
					mime: file.mediaType ?? "application/octet-stream",
					filename: file.filename,
					url: file.url,
				})
			}
			await client.session.promptAsync({
				sessionID: agent.sessionId,
				parts,
				model: effectiveModel
					? { providerID: effectiveModel.providerID, modelID: effectiveModel.modelID }
					: undefined,
				agent: selectedAgent || undefined,
				variant: selectedVariant,
			})
		},
		[agent.directory, agent.sessionId, effectiveModel, selectedAgent, selectedVariant],
	)

	const handleSend = useCallback(
		async (text: string, files?: FileAttachment[]) => {
			log.debug("handleSend called", {
				textLength: text.trim().length,
				hasOnSendMessage: !!onSendMessage,
				sending,
				sessionId: agent.sessionId,
			})
			if (!text.trim() || (!onSendMessage && !activeTrigger) || sending) {
				log.warn("handleSend bailed", {
					emptyText: !text.trim(),
					noOnSendMessage: !onSendMessage,
					sending,
				})
				return
			}

			if (!activeTrigger && text.trim().startsWith("/")) {
				const handled = await handleSlashCommand(text)
				if (handled) {
					slashCommandRef.current?.setText("")
					clearDraft()
					setMentions([])
					return
				}
			}

			setSending(true)
			try {
				if (effectiveModel && agent.directory) {
					appStore.set(setProjectModelAtom, {
						directory: agent.directory,
						model: {
							...effectiveModel,
							variant: selectedVariant,
							agent: selectedAgent || undefined,
						},
					})
				}

				log.debug("handleSend calling onSendMessage", {
					sessionId: agent.sessionId,
					directory: agent.directory,
					model: effectiveModel,
					agentName: selectedAgent,
					variant: selectedVariant,
					hasFiles: !!(files && files.length > 0),
				})

				// Prepend diff comments as structured context if any exist
				const commentPrefix = serializeCommentsForChat(diffComments)
				const finalText = commentPrefix ? `${commentPrefix}${text.trim()}` : text.trim()

				if (activeTrigger) {
					const trigger = activeTrigger
					await submitTriggeredPrompt(trigger, finalText, files)
					log.debug("handleSend triggered prompt completed", {
						sessionId: agent.sessionId,
						trigger,
					})
					if (trigger === "goal") {
						setTimeout(() => void refreshGoalStatus(), 400)
						setTimeout(() => void refreshGoalStatus(), 1_200)
					}
				} else {
					await onSendMessage?.(agent, finalText, {
						model: effectiveModel ?? undefined,
						agentName: selectedAgent || undefined,
						variant: selectedVariant,
						files,
						collaborationMode,
					})
					log.debug("handleSend onSendMessage completed", { sessionId: agent.sessionId })
				}
				clearDraft()
				setMentions([])
				setActiveTrigger(null)
				// Clear diff comments after successful send
				if (diffComments.length > 0) {
					setDiffComments([])
				}
				requestAnimationFrame(() => {
					scrollRef.current?.scrollToBottom("smooth")
				})
			} catch (err) {
				log.error("handleSend failed", { sessionId: agent.sessionId }, err)
			} finally {
				setSending(false)
			}
		},
		[
			onSendMessage,
			sending,
			agent,
			effectiveModel,
			selectedAgent,
			selectedVariant,
			clearDraft,
			activeTrigger,
			submitTriggeredPrompt,
			refreshGoalStatus,
			handleSlashCommand,
			scrollRef,
			diffComments,
			setDiffComments,
			collaborationMode,
		],
	)

	const canSend = isConnected && !sending

	const handleStop = useCallback(() => {
		if (onStop && isWorking) {
			onStop(agent)
		}
	}, [onStop, isWorking, agent])

	const handleEscapeAbort = useCallback(() => {
		if (!isWorking) return

		setInterruptCount((prev) => {
			const next = prev + 1
			if (next >= 2) {
				handleStop()
				if (interruptTimerRef.current) clearTimeout(interruptTimerRef.current)
				return 0
			}
			if (interruptTimerRef.current) clearTimeout(interruptTimerRef.current)
			interruptTimerRef.current = setTimeout(() => setInterruptCount(0), 3000)
			return next
		})
	}, [isWorking, handleStop])

	// --- Popover state (slash commands + mentions) ---
	const [slashOpen, setSlashOpen] = useState(false)
	const [slashQuery, setSlashQuery] = useState("")
	const [mentionOpen, setMentionOpen] = useState(false)
	const [mentionQuery, setMentionQuery] = useState("")



	const slashPopoverRef = useRef<SlashCommandPopoverHandle>(null)
	const mentionPopoverRef = useRef<MentionPopoverHandle>(null)

	const handleSlashTriggerChange = useCallback((open: boolean, query: string) => {
		setSlashOpen(open)
		setSlashQuery(query)
	}, [])

	const handleMentionTriggerChange = useCallback((open: boolean, query: string) => {
		setMentionOpen(open)
		setMentionQuery(query)
	}, [])

	const handleSlashClose = useCallback(() => {
		setSlashOpen(false)
		setSlashQuery("")
	}, [])

	const handleMentionClose = useCallback(() => {
		setMentionOpen(false)
		setMentionQuery("")
	}, [])

	const handleSlashSelect = useCallback(
		(command: string) => {
			handleSlashClose()
			const ctrl = slashCommandRef.current
			// Use the command string directly instead of setText + getText round-trip,
			// which races with React's asynchronous state batching and sometimes reads
			// stale text (e.g. "/un" instead of "/undo").
			if (command.startsWith("/")) {
				handleSlashCommand(command).then((handled) => {
					if (handled) {
						if (ctrl) ctrl.setText("")
						clearDraft()
					} else if (ctrl) {
						// Not a recognized command — leave it in the input for the user
						ctrl.setText(command)
					}
				})
			} else if (ctrl) {
				ctrl.setText(command)
			}
		},
		[handleSlashClose, handleSlashCommand, clearDraft],
	)

	const handleMentionSelect = useCallback(
		(option: MentionOption) => {
			handleMentionClose()
			const ctrl = slashCommandRef.current
			if (!ctrl) return

			const currentText = ctrl.getText()
			const textarea = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
			const cursorPos = textarea?.selectionStart ?? currentText.length

			const mention = createMentionFromOption(option)

			const { text: newText, cursorPosition: newCursor } = insertMentionIntoText(
				currentText,
				cursorPos,
				mention,
			)

			ctrl.setText(newText)

			setMentions((prev) => {
				const key = getMentionKey(mention)
				if (prev.some((candidate) => getMentionKey(candidate) === key))
					return prev
				return [...prev, mention]
			})

			requestAnimationFrame(() => {
				const ta = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
				if (ta) {
					ta.focus()
					ta.setSelectionRange(newCursor, newCursor)
				}
			})
		},
		[handleMentionClose],
	)

	const handleMentionRemove = useCallback((mention: PromptMention) => {
		const ctrl = slashCommandRef.current
		if (ctrl) {
			const marker = getMentionMarker(mention)
			const currentText = ctrl.getText()
			ctrl.setText(currentText.replace(`${marker} `, "").replace(marker, ""))
		}
		setMentions((prev) => {
			const key = getMentionKey(mention)
			return prev.filter((candidate) => getMentionKey(candidate) !== key)
		})
	}, [])

	const handleTextareaKeyDown = useCallback(
		(e: React.KeyboardEvent<HTMLTextAreaElement>) => {
			if (e.key === "Tab" && e.shiftKey) {
				e.preventDefault()
				handleSlashClose()
				handleMentionClose()
				setCollaborationMode((current) => (current === "plan" ? "build" : "plan"))
				return
			}

			// Always delegate to popovers first — they guard on their own `open` prop
			// internally, so we don't need to check slashOpen/mentionOpen here.
			// This avoids stale-closure issues where the parent's boolean lags behind
			// the popover's actual state (due to async TriggerDetector effects).
			if (slashPopoverRef.current?.handleKeyDown(e)) return
			if (mentionPopoverRef.current?.handleKeyDown(e)) return

			if (e.key === "Escape") {
				handleEscapeAbort()
			}
		},
		[handleEscapeAbort, handleSlashClose, handleMentionClose, setCollaborationMode],
	)

	// Width constraint class: remove max-w when review panel is open
	const inputWidthClass = reviewPanelOpen
		? "mx-auto w-full min-w-0"
		: "mx-auto w-full min-w-0 max-w-3xl"

	return (
		<>
			<div className="pointer-events-none min-w-0 px-6 pb-4 pt-0 sm:px-8 lg:px-10">
				<div className={cn(inputWidthClass, "pointer-events-auto")}>
					{/* Session task list — collapsible todo progress */}
					<SessionTaskList sessionId={agent.sessionId} />

					{/* Revert banner — shown when session is in undo state */}
					{isReverted && (
						<div className="mb-2 flex items-center gap-2 rounded-lg border border-amber-400/30 bg-amber-50 px-3 py-2 text-xs text-amber-800 dark:border-amber-500/20 dark:bg-amber-500/5 dark:text-amber-400">
							<Undo2Icon className="size-3.5 shrink-0" />
							<span className="flex-1">
								Session reverted — type to continue from here, or redo to restore
							</span>
							{canRedo && onRedo && (
								<button
									type="button"
									onClick={() => onRedo()}
									className="flex items-center gap-1 rounded-md bg-amber-200/60 px-2 py-1 text-[11px] font-medium text-amber-900 transition-colors hover:bg-amber-200 dark:bg-amber-500/10 dark:text-amber-300 dark:hover:bg-amber-500/20"
								>
									<Redo2Icon className="size-3" />
									Redo
								</button>
							)}
						</div>
					)}

					{/* Pending permissions — tree-scoped: shows own OR any sub-agent's permission */}
					{turns.length === 0 && effectivePermission && (
						<div className="pb-2">
								<PermissionItem
									key={effectivePermission.request.id}
									agent={agent}
									permission={effectivePermission.request}
									onApprove={handleApprovePermission}
									onDeny={handleDenyPermission}
									isConnected={isConnected}
									isFromSubAgent={effectivePermission.sessionId !== agent.sessionId}
								/>
						</div>
					)}

					{/* When questions are pending, replace the input with a focused question flow.
					    Tree-scoped: shows own OR any sub-agent's question. */}
					{effectiveQuestion ? (
						<ChatQuestionFlow
							questions={[effectiveQuestion.request]}
							isFromSubAgent={effectiveQuestion.sessionId !== agent.sessionId}
							onReply={handleReplyQuestion}
							onReject={handleRejectQuestion}
							disabled={!isConnected}
						/>
					) : (
						/* Input card — PromptInputProvider wraps everything,
					   popovers positioned relative to the card wrapper,
					   textarea as a direct child of InputGroup inside PromptInput */
						<PromptInputProvider key={agent.sessionId} initialInput={draft}>
							<DraftSync setDraft={setDraft} />
							<SlashCommandBridge controllerRef={slashCommandRef} />
							<TriggerDetector
								onSlashChange={handleSlashTriggerChange}
								onMentionChange={handleMentionTriggerChange}
							/>
							<MentionReconciler mentions={mentions} onReconcile={setMentions} />
							{/* Relative wrapper for absolutely-positioned popovers */}
							<div className="relative">
								{/* Popovers render above the card via bottom-full */}
							<SlashCommandPopover
								ref={slashPopoverRef}
								query={slashQuery}
								open={slashOpen}
								enabled={isConnected}
								onSelect={handleSlashSelect}
								onClose={handleSlashClose}
							/>
								<MentionPopover
									ref={mentionPopoverRef}
									query={mentionQuery}
									open={mentionOpen}
									directory={agent.directory}
									agents={devoAgents ?? []}
									onSelect={handleMentionSelect}
									onClose={handleMentionClose}
								/>
								<ComposerStatusStack
									goal={activeGoal}
									goalAction={goalAction}
									onEditGoal={handleEditGoal}
									onPauseGoal={handlePauseGoal}
									onResumeGoal={handleResumeGoal}
									onClearGoal={handleClearGoal}
								/>
								<PromptInput
									className={cn(
										"devo-composer bg-background/95 shadow-[0_8px_32px_rgba(0,0,0,0.05)] dark:shadow-[0_10px_36px_rgba(0,0,0,0.28)]",
										activeGoal && "rounded-t-none border-t-border/70",
									)}
									accept="image/png,image/jpeg,image/gif,image/webp,application/pdf"
									multiple
									maxFileSize={10 * 1024 * 1024}
									onSubmit={(message) => {
										if (message.text.trim() && canSend)
											handleSend(message.text, message.files.length > 0 ? message.files : undefined)
									}}
								>
									{/* Mention chips above the textarea */}
									<ContextItems mentions={mentions} onRemove={handleMentionRemove} />
									{/* Diff comment chips above the textarea */}
									{diffComments.length > 0 && (
										<DiffCommentChips
											comments={diffComments}
											onRemove={(id) => setDiffComments((prev) => prev.filter((c) => c.id !== id))}
										/>
									)}
									<PromptAttachmentPreview
										supportsImages={modelCapabilities?.image}
										supportsPdf={modelCapabilities?.pdf}
									/>
									<PromptInputTextarea
										data-prompt-input
										onKeyDown={handleTextareaKeyDown}
										disabled={!isConnected}
										placeholder={
											isWorking ? "Send a follow-up message..." : "What would you like to do?"
										}
									/>

									{/* Toolbar inside the card — agent + model + variant selectors + submit */}
									<PromptInputFooter>
										<PromptInputTools>
											<AttachButton disabled={!isConnected} />
											<PromptToolbar
												agents={devoAgents ?? []}
												selectedAgent={selectedAgent}
												defaultAgent={config?.defaultAgent}
												onSelectAgent={setSelectedAgent}
												providers={providers ?? null}
												effectiveModel={effectiveModel}
												hasModelOverride={!!selectedModel}
												onSelectModel={handleModelSelect}
												selectedVariant={selectedVariant}
												onSelectVariant={handleVariantSelect}
												disabled={!isConnected}
											/>
											<button
												type="button"
												onClick={() =>
													setCollaborationMode(collaborationMode === "plan" ? "build" : "plan")
												}
												disabled={!isConnected}
												className={cn(
													"flex h-7 items-center gap-1 rounded-md px-2 text-xs transition-colors",
													collaborationMode === "plan"
														? "bg-amber-500/15 text-amber-700 dark:text-amber-300"
														: "text-muted-foreground hover:bg-muted hover:text-foreground",
												)}
												title="Toggle plan mode"
											>
												<ListTodoIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
												<span className="capitalize">{collaborationMode}</span>
											</button>
											{activeTrigger && (
												<ComposerTriggerChip
													trigger={activeTrigger}
													onRemove={() => setActiveTrigger(null)}
												/>
											)}
										</PromptInputTools>
										<PromptInputSubmit
											disabled={!canSend}
											status={isWorking ? "streaming" : undefined}
											onStop={handleStop}
											size={isWorking && currentTurnWorkSplit ? "xs" : "icon-sm"}
										>
											{isWorking && currentTurnWorkSplit ? (
												<LiveTurnTimer
													completedMs={currentTurnWorkSplit.completedMs}
													activeStartMs={currentTurnWorkSplit.activeStartMs}
												/>
											) : undefined}
										</PromptInputSubmit>
									</PromptInputFooter>
								</PromptInput>
							</div>
						</PromptInputProvider>
					)}

				</div>
			</div>
			<SkillPickerDialog
				directory={agent.directory}
				onOpenChange={setSkillPickerOpen}
				onSelect={(skillName) => {
					const ctrl = slashCommandRef.current
					if (!ctrl) return
					const current = ctrl.getText()
					const insertion = `$${skillName} `
					ctrl.setText(current.trim() ? `${current.replace(/\/skills\s*$/i, "").trimEnd()} ${insertion}` : insertion)
				}}
				open={skillPickerOpen}
			/>

		</>
	)
}

// ============================================================
// Live turn timer — ticks every second while the agent is working
// ============================================================

/**
 * Compact live timer that shows elapsed time from the user prompt to now.
 */
function LiveTurnTimer({
	completedMs,
	activeStartMs,
}: {
	completedMs: number
	activeStartMs: number | null
}) {
	const computeDisplay = useCallback(
		() =>
			formatWorkDuration(completedMs + (activeStartMs != null ? Date.now() - activeStartMs : 0)),
		[completedMs, activeStartMs],
	)

	const [elapsed, setElapsed] = useState(computeDisplay)

	useEffect(() => {
		const tick = () => setElapsed(computeDisplay())
		tick()
		// Only tick if there's an active (in-progress) message
		if (activeStartMs != null) {
			const id = setInterval(tick, 1_000)
			return () => clearInterval(id)
		}
	}, [computeDisplay, activeStartMs])

	return (
		<span className="inline-flex items-center gap-1.5 text-xs tabular-nums">
			<SquareIcon className="size-3.5" />
			{elapsed}
		</span>
	)
}

// ============================================================
// Worktree setup progress (shown in empty state during creation)
// ============================================================

const SETUP_PHASE_LABELS: Record<NonNullable<SessionSetupPhase>, string> = {
	"creating-worktree": "Creating worktree...",
	"starting-session": "Starting session...",
}

function WorktreeSetupProgress({ phase }: { phase: NonNullable<SessionSetupPhase> }) {
	return (
		<div className="flex flex-col items-center justify-center gap-4 py-16">
			<div className="flex size-12 items-center justify-center rounded-xl border border-border/50 bg-muted/30">
				<GitForkIcon className="size-5 text-muted-foreground" />
			</div>
			<div className="flex flex-col items-center gap-2">
				<div className="flex items-center gap-2">
					<Loader2Icon className="size-4 animate-spin text-muted-foreground" />
					<p className="text-sm font-medium text-foreground">{SETUP_PHASE_LABELS[phase]}</p>
				</div>
				<p className="text-xs text-muted-foreground">
					Setting up an isolated workspace for this session
				</p>
			</div>
		</div>
	)
}

// ============================================================
// Diff comment chips shown above the chat input
// ============================================================

function DiffCommentChips({
	comments,
	onRemove,
}: {
	comments: DiffComment[]
	onRemove: (id: string) => void
}) {
	if (comments.length === 0) return null

	return (
		<div className="flex flex-wrap gap-1 px-1 pt-1">
			{comments.map((comment) => {
				const fileName = comment.filePath.split("/").pop() ?? comment.filePath
				return (
					<span
						key={comment.id}
						className="inline-flex max-w-full items-center gap-1 rounded-md border border-primary/20 bg-primary/5 px-1.5 py-0.5 text-[10px] leading-tight"
					>
						<span className="shrink-0 font-mono text-muted-foreground">
							{fileName}:{comment.lineNumber}
						</span>
						<span className="truncate text-foreground">
							{comment.content.length > 40 ? `${comment.content.slice(0, 40)}...` : comment.content}
						</span>
						<button
							type="button"
							onClick={() => onRemove(comment.id)}
							className="shrink-0 text-muted-foreground/60 hover:text-foreground"
						>
							<XIcon className="size-2.5" />
						</button>
					</span>
				)
			})}
		</div>
	)
}
