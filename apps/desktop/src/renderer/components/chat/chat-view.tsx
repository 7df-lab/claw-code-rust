import { useTurnRecovery } from "../../hooks/use-turn-recovery"
import { TurnRecoveryPanel } from "./turn-recovery-panel"
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
import { cn } from "@devo/ui/lib/utils"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useAtom, useAtomValue, useSetAtom } from "jotai"
import {
	GitForkIcon,
	Loader2Icon,
	PlusIcon,
	Redo2Icon,
	Undo2Icon,
	XIcon,
} from "lucide-react"
import {
	type CSSProperties,
	Fragment,
	type ReactNode,
	type MutableRefObject,
	type RefObject,
	useCallback,
	useEffect,
	useImperativeHandle,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
} from "react"
import { collaborationModeFamily, type CollaborationMode } from "../../atoms/collaboration-mode"
import { compactionStatusFamily } from "../../atoms/compaction"
import { messagesFamily } from "../../atoms/messages"
import { projectModelsAtom, setProjectModelAtom } from "../../atoms/preferences"
import {
	composerFromSessionModel,
	hydrateSessionComposerState,
	sessionComposerFamily,
	setSessionComposerAtom,
} from "../../atoms/session-composer"
import type { ProviderErrorEntry, ProviderRetryStatus, SessionSetupPhase } from "../../atoms/sessions"
import { sessionFamily } from "../../atoms/sessions"
import {
	effectivePermissionFamily,
	effectiveQuestionFamily,
} from "../../atoms/derived/session-requests"
import { appStore } from "../../atoms/store"
import { sessionScrollSnapshotFamily, settingsOverlayOpenAtom } from "../../atoms/ui"
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
import {
	isRestoringSessionScroll,
	planSessionScrollRestore,
	restoreSessionScrollWhenReady,
	snapshotFromScrollElement,
} from "../../lib/session-scroll-restore"
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
	modelRefFromSlug,
	resolveEffectiveModel,
	useModelState,
} from "../../hooks/use-devo-data"
import type { ChatTurn } from "../../hooks/use-session-chat"
import { warmDiffHighlighter } from "../../lib/diff-highlighter-warmup"
import { createLogger } from "../../lib/logger"
import type { Agent, FileAttachment, PermissionResponse, QuestionAnswer } from "../../lib/types"
import { getBaseClient, getProjectClient } from "../../services/connection-manager"

const log = createLogger("chat-view")

/** Session ids whose collaboration mode was already seeded from persisted settings this run. */
const seededCollaborationModes = new Set<string>()

/** Debounce for persist-on-selection: coalesces rapid model/effort/mode changes. */
const SELECTION_PERSIST_DEBOUNCE_MS = 500

const VIRTUALIZE_TURN_THRESHOLD = 30
const VIRTUAL_TURN_GAP = 40

/** Stable empty array so historical ChatTurn memo is not busted every stream tick. */
const EMPTY_PROVIDER_ERRORS: ProviderErrorEntry[] = []

import {
	type DiffComment,
	diffCommentsFamily,
	serializeCommentsForChat,
} from "../review/review-comments"
import { ChatPermissionFlow } from "./chat-permission"
import { ChatQuestionFlow } from "./chat-question"
import { ChatTurnComponent, isSyntheticMessage } from "./chat-turn"
import { ForkBoundaryDivider } from "./fork-boundary-divider"
import { forkBoundaryAfterTurnIndex } from "./fork-boundary"
import { ChatLoadingSkeleton } from "./chat-turn-skeleton"
import { ProviderErrorRow } from "./provider-error-row"
import {
	ComposerStatusStack,
	type ComposerGoal,
	type ComposerGoalStatus,
	type ComposerQueueItem,
} from "./composer-status-stack"
import { useComposerQueue } from "../../hooks/use-composer-queue"
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
import { ComposerModeChip } from "./composer-mode-chip"
import { ComposerPermissionPicker } from "./composer-permission-picker"
import {
	DEFAULT_COMPOSER_PERMISSION_PROFILE,
	type ComposerPermissionProfile,
	parseComposerPermissionProfile,
	takeComposerPermissionForSession,
} from "./composer-permission"
import { goalPromptText, parseComposerSlash } from "./composer-slash"
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
			className="size-8 rounded-full bg-muted/80 text-muted-foreground hover:bg-muted hover:text-foreground"
		>
			<PlusIcon className="size-4" />
		</PromptInputButton>
	)
}

/**
 * Restores scroll position when session content finishes loading or the session
 * remounts after LRU eviction. Respects saved scrollTop unless the user was at bottom.
 */
function ScrollOnLoad({
	loading,
	sessionId,
	isActive,
}: {
	loading: boolean
	sessionId: string
	isActive: boolean
}) {
	const { scrollToBottom, scrollRef, stopScroll } = useStickToBottomContext()
	const settingsOverlayOpen = useAtomValue(settingsOverlayOpenAtom)
	const prevLoadingRef = useRef(loading)
	const prevActiveRef = useRef(isActive)

	useLayoutEffect(() => {
		const wasLoading = prevLoadingRef.current
		const becameActive = !prevActiveRef.current && isActive
		prevLoadingRef.current = loading
		prevActiveRef.current = isActive

		if (!isActive || settingsOverlayOpen || getPendingRestoreScrollTop() != null) return

		if ((wasLoading && !loading) || becameActive) {
			const snapshot = appStore.get(sessionScrollSnapshotFamily(sessionId))
			const plan = planSessionScrollRestore(snapshot)
			if (plan.action === "bottom") {
				scrollToBottom("instant")
			} else {
				markScrollRestored(plan.scrollTop)
				restoreSessionScrollWhenReady({
					sessionId,
					getElement: () => scrollRef.current,
					scrollTop: plan.scrollTop,
					stopScroll,
					onRestored: markScrollRestored,
				})
			}
		}
	}, [loading, sessionId, isActive, scrollToBottom, scrollRef, stopScroll, settingsOverlayOpen])

	return null
}

/**
 * Tracks scroll position while the session is visible so it can be restored
 * after returning from Settings (StickToBottom may reset on layout changes).
 */
function ScrollPositionTracker({
	sessionId,
	isActive,
}: {
	sessionId: string
	isActive: boolean
}) {
	const { scrollRef } = useStickToBottomContext()
	const setSnapshot = useSetAtom(sessionScrollSnapshotFamily(sessionId))
	const settingsOverlayOpen = useAtomValue(settingsOverlayOpenAtom)

	useEffect(() => {
		const element = scrollRef.current
		if (!element || !isActive) return

		const onScroll = () => {
			if (settingsOverlayOpen) return
			setSnapshot(snapshotFromScrollElement(element))
		}

		element.addEventListener("scroll", onScroll, { passive: true })
		return () => element.removeEventListener("scroll", onScroll)
	}, [scrollRef, sessionId, isActive, setSnapshot, settingsOverlayOpen])

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

export interface ChatScrollHandle {
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
 * Also exposes scroll position helpers for load-earlier anchor restore.
 */
function ScrollBridge({ scrollRef }: { scrollRef: React.RefObject<ChatScrollHandle | null> }) {
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
	scrollRef: RefObject<ChatScrollHandle | null>
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

function turnListStructureRevision(turns: ChatTurn[]): string {
	return turns
		.map((turn) => {
			let partCount = turn.userMessage.parts.length
			for (const message of turn.assistantMessages) {
				partCount += message.parts.length
			}
			return `${turn.id}:${partCount}`
		})
		.join("|")
}

interface VirtualizedTurnListProps {
	turns: ChatTurn[]
	renderTurn: (turn: ChatTurn, index: number) => ReactNode
	sessionId: string
}

function VirtualizedTurnList({ turns, renderTurn, sessionId }: VirtualizedTurnListProps) {
	const { scrollRef } = useStickToBottomContext()
	const turnsRevision = useMemo(() => turnListStructureRevision(turns), [turns])
	const virtualizer = useVirtualizer({
		count: turns.length,
		getScrollElement: () => scrollRef.current,
		getItemKey: (index) => turns[index]?.id ?? index,
		estimateSize: (index) => estimateTurnSize(turns[index]),
		overscan: 5,
	})

	useLayoutEffect(() => {
		if (!isRestoringSessionScroll(sessionId)) {
			virtualizer.measure()
			return
		}
		const pending = getPendingRestoreScrollTop()
		const snapshot = appStore.get(sessionScrollSnapshotFamily(sessionId))
		const plan = planSessionScrollRestore(
			pending != null
				? { scrollTop: pending, atBottom: false, hasSnapshot: true }
				: snapshot,
		)
		if (plan.action === "restore" && scrollRef.current) {
			scrollRef.current.scrollTop = plan.scrollTop
		}
		virtualizer.measure()
	}, [turnsRevision, virtualizer, scrollRef, sessionId])

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
	/** True when fetching with no cached turns to display yet. */
	showLoading?: boolean
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
	onDeny?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		note?: string,
	) => Promise<void>
	/** Question handlers */
	onReplyQuestion?: (agent: Agent, requestId: string, answers: QuestionAnswer[]) => Promise<void>
	onRejectQuestion?: (agent: Agent, requestId: string) => Promise<void>
	/** Undo/redo */
	canUndo?: boolean
	canRedo?: boolean
	onUndo?: () => Promise<string | undefined>
	onRedo?: () => Promise<void>
	isReverted?: boolean
	/** Fork from a turn boundary (protocol turn id, or undefined for tip fork) */
	onForkFromTurn?: (turnId?: string) => Promise<void>
	/** Edit and resend the latest user message */
	onEditUserMessage?: (messageId: string, text: string) => Promise<void>
	/** Delete a specific part from a message (for error recovery) */
	onDeletePart?: (sessionId: string, messageId: string, partId: string) => Promise<void>
	/** Whether the review panel is open (removes max-w constraint) */
	reviewPanelOpen?: boolean
	/** Parent session title for fork boundary marker */
	parentSessionName?: string
	/** Whether this session is the visible panel (gates scroll tracking). */
	isActive?: boolean
	/** When false, only render the transcript (used by SessionShell). */
	showComposer?: boolean
	/** When false, only render the composer (used by SessionShell). */
	showTranscript?: boolean
	/** Shared scroll handle when composer is rendered outside the transcript. */
	externalScrollRef?: RefObject<ChatScrollHandle | null>
	/** Composer height when rendered outside this ChatView instance. */
	composerInsetPx?: number
	/** Registers /side handler when transcript and composer are split. */
	sideQuestionHandlerRef?: MutableRefObject<((question: string) => Promise<void>) | null>
}

type SideCard = {
	id: string
	question: string
	answer: string
	status: "running" | "done" | "failed"
}

type SelectionPersistPatch = {
	modelID?: string
	reasoningEffort?: string
	mode?: string
	permissionProfile?: string
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
	showLoading = false,
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
	onForkFromTurn,
	onEditUserMessage,
	onDeletePart,
	reviewPanelOpen,
	parentSessionName,
	isActive = true,
	showComposer = true,
	showTranscript = true,
	externalScrollRef,
	composerInsetPx,
	sideQuestionHandlerRef,
}: ChatViewProps) {
	const recoveryState = useTurnRecovery(agent.sessionId, agent.directory, agent.status)
	const isWorking = agent.status === "running" && !recoveryState.recovery
	const settingsOverlayOpen = useAtomValue(settingsOverlayOpenAtom)

	useEffect(() => {
		void warmDiffHighlighter().catch(() => {
			// Best-effort warmup; inline diffs retry via PierreDiffMount remount.
		})
	}, [])

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
	const internalScrollRef = useRef<ChatScrollHandle | null>(null)
	const scrollRef = externalScrollRef ?? internalScrollRef
	const composerRef = useRef<HTMLDivElement | null>(null)
	const [measuredComposerInset, setMeasuredComposerInset] = useState(0)
	const composerInset = composerInsetPx ?? measuredComposerInset

	// Session-level error and setup phase from the session atom
	const sessionEntry = useAtomValue(sessionFamily(agent.sessionId))
	const sessionError = sessionEntry?.error
	const setupPhase = sessionEntry?.setupPhase
	const compactionStatus = useAtomValue(compactionStatusFamily(agent.sessionId))
	const [sideCards, setSideCards] = useState<SideCard[]>([])

	const startSideQuestion = useCallback(
		async (question: string) => {
			if (!agent.directory) return
			const client = getProjectClient(agent.directory)
			if (!client?.task?.startAgent) {
				log.error("task.startAgent unavailable", { sessionId: agent.sessionId })
				return
			}
			const cardId = crypto.randomUUID()
			setSideCards((prev) => [
				...prev,
				{ id: cardId, question, answer: "", status: "running" },
			])
			const prompt =
				"You are answering a /side side question in a lightweight forked agent.\n" +
				"The inherited conversation is reference context only. Do not continue or modify the " +
				"main session task. Answer only this side question.\n" +
				"You cannot use tools in this fork: do not read files, run commands, search, or modify code. " +
				"Produce one concise answer and stop.\n\n" +
				`Side question:\n${question}`
			try {
				const result = await client.task.startAgent({
					sessionID: agent.sessionId,
					prompt,
					forkTurns: "all",
					maxTurns: 1,
					toolPolicy: "deny_all",
					ephemeral: true,
				})
				const itemId = result.data.itemId
				const childSessionId = itemId.startsWith("item_") ? itemId.slice("item_".length) : itemId
				let answer = ""
				for (let attempt = 0; attempt < 40; attempt++) {
					await new Promise((resolve) => setTimeout(resolve, 250))
					try {
						const messages = await client.session.messages({
							sessionID: childSessionId,
							limit: 50,
						})
						const texts: string[] = []
						for (const entry of messages.data ?? []) {
							if (entry.info.role !== "assistant") continue
							for (const part of entry.parts ?? []) {
								if (part.type === "text" && typeof part.text === "string" && part.text.trim()) {
									texts.push(part.text)
								}
							}
						}
						answer = texts.join("\n").trim()
						if (answer) break
					} catch {
						// Child may still be starting; keep polling.
					}
				}
				setSideCards((prev) =>
					prev.map((card) =>
						card.id === cardId
							? {
									...card,
									answer: answer || "No answer returned.",
									status: answer ? "done" : "failed",
								}
							: card,
					),
				)
			} catch (err) {
				log.error("slash /side failed", { sessionId: agent.sessionId }, err)
				setSideCards((prev) =>
					prev.map((card) =>
						card.id === cardId
							? {
									...card,
									answer: err instanceof Error ? err.message : "Side question failed.",
									status: "failed",
								}
							: card,
					),
				)
			}
		},
		[agent.directory, agent.sessionId],
	)

	// Clear ephemeral side cards when switching sessions.
	useEffect(() => {
		setSideCards([])
	}, [agent.sessionId])

	useEffect(() => {
		if (!sideQuestionHandlerRef || !isActive || showComposer) return
		sideQuestionHandlerRef.current = startSideQuestion
		return () => {
			if (sideQuestionHandlerRef.current === startSideQuestion) {
				sideQuestionHandlerRef.current = null
			}
		}
	}, [isActive, showComposer, sideQuestionHandlerRef, startSideQuestion])

	useLayoutEffect(() => {
		if (composerInsetPx != null || !showComposer) {
			return
		}
		if (setupPhase) {
			setMeasuredComposerInset(0)
			return
		}

		const composer = composerRef.current
		if (!composer) return

		const updateComposerInset = () => {
			const nextInset = Math.ceil(composer.getBoundingClientRect().height)
			setMeasuredComposerInset((currentInset) =>
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
	}, [composerInsetPx, setupPhase, showComposer])
	const effectivePermission = useAtomValue(effectivePermissionFamily(agent.sessionId))

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

	const showSessionError =
		!!sessionErrorText && !lastTurnHasError && (sessionEntry?.providerErrors?.length ?? 0) === 0

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
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onApprove],
	)

	const handleDenyPermission = useCallback(
		async (a: Agent, permissionSessionId: string, permissionId: string, note?: string) => {
			await onDeny?.(a, permissionSessionId, permissionId, note)
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onDeny],
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
	const providerErrors = sessionEntry?.providerErrors ?? EMPTY_PROVIDER_ERRORS
	const lastTurnProviderErrors = useMemo(() => {
		if (providerErrors.length === 0) return EMPTY_PROVIDER_ERRORS
		const lastTurn = turns[turns.length - 1]
		if (!lastTurn?.turnId) return providerErrors
		const filtered = providerErrors.filter(
			(entry) => !entry.turnId || entry.turnId === lastTurn.turnId,
		)
		return filtered.length === providerErrors.length ? providerErrors : filtered
	}, [providerErrors, turns])
	const latestEditableUserTurnIndex = useMemo(() => {
		for (let index = turns.length - 1; index >= 0; index--) {
			if (!isSyntheticMessage(turns[index].userMessage)) return index
		}
		return -1
	}, [turns])

	const forkBoundaryAfterIndex = useMemo(
		() =>
			forkBoundaryAfterTurnIndex(
				turns,
				agent.forkFromId,
				agent.atTurnId,
				agent.createdAt,
			),
		[agent.atTurnId, agent.createdAt, agent.forkFromId, turns],
	)

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
			const turnProviderErrors = !isLastTurn ? EMPTY_PROVIDER_ERRORS : lastTurnProviderErrors
			return (
			<Fragment key={turn.id}>
			<ChatTurnComponent
				turn={turn}
				isLast={isLastTurn}
				isWorking={isWorking}
				agent={agent}
				isConnected={isConnected}
				compactionStatus={compactionStatus}
				retryStatus={activeRetryStatus}
				providerErrors={turnProviderErrors}
				onForkFromTurn={
					onForkFromTurn
						? () => onForkFromTurn(turn.turnId)
						: undefined
				}
				onEditUserMessage={
					index === latestEditableUserTurnIndex && onEditUserMessage
						? (text) => onEditUserMessage(turn.userMessage.info.id, text)
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
			{index === forkBoundaryAfterIndex ? (
				<ForkBoundaryDivider
					parentName={parentSessionName}
					sourceSessionId={agent.forkFromId}
					projectSlug={agent.projectSlug}
				/>
			) : null}
			</Fragment>
			)
		},
		[
			agent,
			effectivePermission,
			compactionStatus,
			forkBoundaryAfterIndex,
			handleApprovePermission,
			handleDenyPermission,
			isConnected,
			isWorking,
			latestEditableUserTurnIndex,
			onDeletePart,
			onForkFromTurn,
			onEditUserMessage,
			onSendMessage,
			parentSessionName,
			lastTurnProviderErrors,
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
			{showTranscript ? (
			<div
				className="relative min-h-0 min-w-0 flex-1"
				data-conversation-surface={agent.sessionId}
			>
				<Conversation
					key={agent.sessionId}
					className="h-full"
					streaming={isWorking}
					targetScrollTop={conversationTargetScrollTop}
				>
					<ScrollOnLoad loading={loading} sessionId={agent.sessionId} isActive={isActive} />
					<SettingsScrollGuard sessionId={agent.sessionId} />
					<ScrollPositionTracker sessionId={agent.sessionId} isActive={isActive} />
					<ScrollBridge scrollRef={scrollRef} />
					<ConversationContent
						scrollClassName="scrollbar-chat"
						className="gap-12 px-6 pt-4 pb-4 sm:px-10 sm:pt-8 sm:pb-6 lg:px-12"
					>
						<div className={cn(contentWidthClass, "animate-in fade-in space-y-12 duration-150")}>
							<LoadEarlierOnScroll
								hasEarlierMessages={hasEarlierMessages}
								loadingEarlier={loadingEarlier}
								onLoadEarlier={onLoadEarlier}
								scrollRef={scrollRef}
							/>

							{showLoading ? (
								<ChatLoadingSkeleton />
							) : turns.length > 0 ? (
								turns.length > VIRTUALIZE_TURN_THRESHOLD ? (
									<VirtualizedTurnList
										turns={turns}
										renderTurn={renderTurn}
										sessionId={agent.sessionId}
									/>
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

							{sideCards.map((card) => (
								<div
									key={card.id}
									className="rounded-lg border border-border/80 bg-muted/20 px-3 py-2.5 text-sm"
								>
									<div className="mb-1 flex items-center gap-1.5 text-xs font-medium text-muted-foreground">
										<span>Side</span>
										{card.status === "running" ? (
											<Loader2Icon className="size-3 animate-spin" />
										) : null}
									</div>
									<p className="mb-2 text-foreground/90">{card.question}</p>
									{card.answer ? (
										<p className="whitespace-pre-wrap text-muted-foreground">{card.answer}</p>
									) : (
										<p className="text-xs text-muted-foreground">Thinking…</p>
									)}
								</div>
							))}

							{/* Session-level error when no turn-scoped expandable rows exist */}
							{showSessionError && sessionErrorText && (
								<div className="py-0.5">
									<ProviderErrorRow
										entry={{
											id: "session-error",
											turnId: "",
											message: sessionErrorText,
											phase: "failed",
											code: sessionError?.name,
										}}
									/>
								</div>
							)}
						</div>
					</ConversationContent>
					<ConversationScrollButton className="!bottom-3" />
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
					className="pointer-events-none absolute inset-x-0 bottom-0 z-10 h-6 bg-gradient-to-t from-background/30 to-transparent"
				/>
			</div>
			) : null}

			{showComposer && !setupPhase && (
				<div
					ref={composerRef}
					className="pointer-events-none absolute bottom-0 left-0 right-3.5 z-30 overflow-visible pt-3"
				>
					<ChatInputSection
						agent={agent}
						turns={turns}
						isConnected={isConnected}
						isWorking={isWorking}
						recoveryState={recoveryState}
						onSendMessage={onSendMessage}
						onStop={onStop}
						providers={providers}
						config={config}
						devoAgents={devoAgents}
						onApprove={handleApprovePermission}
						onDeny={handleDenyPermission}
						onReplyQuestion={onReplyQuestion}
						onRejectQuestion={onRejectQuestion}
						onForkFromTurn={onForkFromTurn}
						onStartSideQuestion={startSideQuestion}
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
	recoveryState?: ReturnType<typeof useTurnRecovery>
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
	onDeny?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		note?: string,
	) => Promise<void>
	onReplyQuestion?: ChatViewProps["onReplyQuestion"]
	onRejectQuestion?: ChatViewProps["onRejectQuestion"]
	onForkFromTurn?: ChatViewProps["onForkFromTurn"]
	onStartSideQuestion?: (question: string) => Promise<void>
	canRedo?: boolean
	onRedo?: () => Promise<void>
	isReverted?: boolean
	scrollRef: React.RefObject<ChatScrollHandle | null>
	reviewPanelOpen?: boolean
}

export function ChatInputSection({
	recoveryState,
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
	onForkFromTurn,
	onStartSideQuestion,
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
	const {
		queueItems,
		draggingId: draggingQueueItemId,
		setDraggingId: setDraggingQueueItemId,
		steerQueueItem,
		removeQueueItem,
		editQueueItem,
		reorderQueueItem,
	} = useComposerQueue(agent.sessionId, agent.directory ?? null)
	const [collaborationMode, setCollaborationModeAtom] = useAtom(
		collaborationModeFamily(agent.sessionId),
	)
	const [permissionProfile, setPermissionProfile] =
		useState<ComposerPermissionProfile>(DEFAULT_COMPOSER_PERMISSION_PROFILE)

	// User requirement: the /goal footer chip is only an input trigger;
	// the composer-adjacent status row reflects the real session goal state.
	useEffect(() => {
		setActiveTrigger(null)
		setActiveGoal(null)
		setGoalAction(null)
		const pendingPermission = takeComposerPermissionForSession(agent.sessionId)
		if (pendingPermission) {
			setPermissionProfile(pendingPermission)
		}
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

	// Diff comments integration
	const diffComments = useAtomValue(diffCommentsFamily(agent.sessionId))
	const setDiffComments = useSetAtom(diffCommentsFamily(agent.sessionId))

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
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onApprove, scrollRef],
	)

	const handleDenyPermission = useCallback(
		async (a: Agent, permissionSessionId: string, permissionId: string, note?: string) => {
			await onDeny?.(a, permissionSessionId, permissionId, note)
			requestAnimationFrame(() => {
				scrollRef.current?.scrollToBottom("smooth")
			})
		},
		[onDeny, scrollRef],
	)

	// Draft persistence
	const draft = useDraftSnapshot(agent.sessionId)
	const { setDraft, clearDraft } = useDraftActions(agent.sessionId)

	// Escape-to-abort: double-press within 3s
	const [, setInterruptCount] = useState(0)
	const interruptTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

	// Per-session composer settings (model / variant / agent).
	const sessionMessages = useAtomValue(messagesFamily(agent.sessionId))
	const projectModels = useAtomValue(projectModelsAtom)
	const composerState = useAtomValue(sessionComposerFamily(agent.sessionId))
	const setComposerState = useSetAtom(setSessionComposerAtom)
	const hydratedForMessagesRef = useRef<string | null>(null)
	const sessionEntry = useAtomValue(sessionFamily(agent.sessionId))

	useEffect(() => {
		const wireSession = sessionEntry?.session as {
			model?: {
				provider?: string
				model?: string
				reasoningEffort?: string
				reasoning_effort?: string
			}
			settings?: {
				mode?: string
				reasoningEffort?: string
				reasoning_effort?: string
				permissionProfile?: string
				permission_profile?: string
			}
		} | null | undefined
		const persistedReasoningEffort =
			wireSession?.model?.reasoningEffort ??
			wireSession?.model?.reasoning_effort ??
			wireSession?.settings?.reasoningEffort ??
			wireSession?.settings?.reasoning_effort
		// Include the wire seed fingerprint so enrichment (session/resume
		// replaces the cold list snapshot with full model/settings) re-runs
		// the hydration even when the message count has not changed.
		const messageKey = `${agent.sessionId}:${sessionMessages.length}:${
			wireSession?.model?.provider ??
			""
		}|${wireSession?.model?.model ?? ""}|${persistedReasoningEffort ?? ""}|${
			wireSession?.settings?.mode ?? ""
		}|${wireSession?.settings?.permissionProfile ?? wireSession?.settings?.permission_profile ?? ""}`
		if (hydratedForMessagesRef.current === messageKey) return
		const projectDefault = agent.directory ? projectModels[agent.directory] : undefined
		// Persisted per-session turn settings survive restarts (the server
		// restores them on resume); history messages do not carry model
		// metadata, so without this seed every restored session would show
		// the project-default model / reasoning effort.
		const wireModel = wireSession?.model
		const wireModelSeed = wireModel
			? {
					provider: wireModel.provider,
					model: wireModel.model,
					reasoningEffort: persistedReasoningEffort,
			  }
			: undefined
		const sessionSeed = composerFromSessionModel(wireModelSeed, (seed) => {
			const providerList = providers?.providers ?? []
			// Prefer the wire provider id when it names a provider that actually
			// serves the model (session/resume carries a real binding); cold
			// list snapshots may only carry "unknown", so fall back to a
			// reverse lookup by model slug.
			if (seed.provider && seed.provider !== "unknown") {
				const provider = providerList.find((p) => p.id === seed.provider)
				if (provider?.models?.[seed.model ?? ""]) {
					return { providerID: seed.provider, modelID: seed.model ?? "" }
				}
			}
			return seed.model ? modelRefFromSlug(seed.model, providerList) : null
		})
		const next = hydrateSessionComposerState(
			composerState,
			sessionMessages,
			projectDefault,
			sessionSeed,
		)
		if (
			next.model?.providerID !== composerState.model?.providerID ||
			next.model?.modelID !== composerState.model?.modelID ||
			next.variant !== composerState.variant ||
			next.agent !== composerState.agent
		) {
			setComposerState({ sessionId: agent.sessionId, patch: next })
		}
		// Record the guard key only once hydration is conclusive. A wire seed
		// that exists but could not be resolved yet (provider list still
		// loading, slug unmatched against current providers) must retry on the
		// next dep change — recording the key now would lock the composer into
		// the default model/effort forever. Writing the ref does not render,
		// so retries are driven purely by dep changes and cannot loop.
		if (next.model || !wireSession?.model?.model || composerState.hasUserOverride) {
			hydratedForMessagesRef.current = messageKey
		}
		// Seed the collaboration mode (build/plan) once per session per run so
		// restarts restore it without fighting in-run user toggles.
		const wireMode = wireSession?.settings?.mode
		if (
			(wireMode === "plan" || wireMode === "build") &&
			!seededCollaborationModes.has(agent.sessionId)
		) {
			seededCollaborationModes.add(agent.sessionId)
			appStore.set(collaborationModeFamily(agent.sessionId), wireMode)
		}
		const wirePermission =
			wireSession?.settings?.permissionProfile ?? wireSession?.settings?.permission_profile
		if (wirePermission) {
			setPermissionProfile(parseComposerPermissionProfile(wirePermission))
		}
	}, [
		agent.directory,
		agent.sessionId,
		composerState,
		projectModels,
		providers,
		sessionEntry,
		sessionMessages,
		setComposerState,
	])

	const selectedModel = composerState.model
	const selectedAgent = composerState.agent
	const selectedVariant = composerState.variant

	const setSelectedModel = useCallback(
		(model: ModelRef | null) => {
			setComposerState({
				sessionId: agent.sessionId,
				patch: { model, variant: undefined },
				userOverride: true,
			})
		},
		[agent.sessionId, setComposerState],
	)

	const setSelectedAgent = useCallback(
		(agentName: string | null) => {
			setComposerState({
				sessionId: agent.sessionId,
				patch: { agent: agentName },
				userOverride: true,
			})
		},
		[agent.sessionId, setComposerState],
	)

	const setSelectedVariant = useCallback(
		(variant: string | undefined) => {
			setComposerState({
				sessionId: agent.sessionId,
				patch: { variant },
				userOverride: true,
			})
		},
		[agent.sessionId, setComposerState],
	)

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
	}, [selectedVariant, effectiveModel, providers, setSelectedVariant])

	const modelCapabilities = useMemo(
		() => getModelInputCapabilities(effectiveModel, providers?.providers ?? []),
		[effectiveModel, providers],
	)

	// ── Persist-on-selection ─────────────────────────────────────────────
	// Composer selections (model / reasoning effort / mode) are persisted to
	// the session record the moment they change, debounced and coalesced into
	// one session/metadata/update per burst, so they survive a restart even
	// if no message is ever sent. The send path still passes them per turn
	// (and re-persists), acting as a backstop.
	const pendingSelectionPersistRef = useRef<SelectionPersistPatch | null>(null)
	const selectionPersistTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
	const permissionProfileDirtyRef = useRef(false)

	const flushSelectionPersist = useCallback((): Promise<void> => {
		if (selectionPersistTimerRef.current !== null) {
			clearTimeout(selectionPersistTimerRef.current)
			selectionPersistTimerRef.current = null
		}
		const pending: SelectionPersistPatch = {
			...pendingSelectionPersistRef.current,
		}
		if (permissionProfileDirtyRef.current) {
			pending.permissionProfile = permissionProfile
		}
		pendingSelectionPersistRef.current = null
		if (!agent.sessionId) return Promise.resolve()
		const hasPersistableFields = Object.keys(pending).length > 0
		if (!hasPersistableFields) return Promise.resolve()
		try {
			const client =
				(agent.directory ? getProjectClient(agent.directory) : null) ?? getBaseClient()
			if (!client) {
				log.warn("session settings persist skipped: not connected", {
					sessionId: agent.sessionId,
				})
				return Promise.resolve()
			}
			const updateSettings = client.session?.updateSettings
			if (typeof updateSettings !== "function") {
				log.warn("session settings persist skipped: client API unavailable", {
					sessionId: agent.sessionId,
				})
				return Promise.resolve()
			}
			// Resolved (not rejected) when the write lands or has failed and
			// been logged — callers await this to order the flush ahead of a
			// turn without taking on error handling.
			return Promise.resolve()
				.then(() => updateSettings.call(client.session, { sessionID: agent.sessionId, ...pending }))
				.then(() => {
					if (pending.permissionProfile) {
						permissionProfileDirtyRef.current = false
					}
				})
				.catch((error: unknown) => {
					log.warn("session settings persist failed", { sessionId: agent.sessionId }, error)
				})
		} catch (error) {
			log.warn("session settings persist failed", { sessionId: agent.sessionId }, error)
			return Promise.resolve()
		}
	}, [agent.directory, agent.sessionId, permissionProfile])

	const scheduleSelectionPersist = useCallback(
		(patch: SelectionPersistPatch) => {
			pendingSelectionPersistRef.current = { ...pendingSelectionPersistRef.current, ...patch }
			if (selectionPersistTimerRef.current !== null) {
				clearTimeout(selectionPersistTimerRef.current)
			}
			selectionPersistTimerRef.current = setTimeout(
				() => flushSelectionPersist(),
				SELECTION_PERSIST_DEBOUNCE_MS,
			)
		},
		[flushSelectionPersist],
	)

	// Unmount / session switch flushes whatever is still pending.
	useEffect(() => {
		return () => {
			flushSelectionPersist()
		}
	}, [flushSelectionPersist])

	const handleModelSelect = useCallback(
		(model: ModelRef | null) => {
			setSelectedModel(model)
			if (!model) return
			addRecentModel(model)
			scheduleSelectionPersist({ modelID: model.modelID })
		},
		[addRecentModel, scheduleSelectionPersist, setSelectedModel],
	)

	const handleVariantSelect = useCallback(
		(variant: string | undefined) => {
			setSelectedVariant(variant)
			if (typeof variant === "string" && variant.length > 0) {
				scheduleSelectionPersist({ reasoningEffort: variant })
			}
		},
		[scheduleSelectionPersist, setSelectedVariant],
	)

	/** Mode toggle that also persists the choice to the session record. */
	const changeCollaborationMode = useCallback(
		(next: CollaborationMode) => {
			setCollaborationModeAtom(next)
			scheduleSelectionPersist({ mode: next })
		},
		[scheduleSelectionPersist, setCollaborationModeAtom],
	)

	const handlePermissionProfileChange = useCallback(
		(profile: ComposerPermissionProfile) => {
			setPermissionProfile(profile)
			permissionProfileDirtyRef.current = true
			scheduleSelectionPersist({ permissionProfile: profile })
		},
		[scheduleSelectionPersist, agent.sessionId],
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
			const parsed = parseComposerSlash(text)
			if (!parsed) return false

			const { name, args } = parsed

			// Product requirement: Desktop slash commands are limited to first-party
			// entries. Compact executes immediately; Goal becomes a footer trigger
			// chip; /plan switches collaboration mode; Research stays as slash text.
			switch (name) {
				case "compact":
					if (agent.directory) {
						const client = getProjectClient(agent.directory)
						if (client) {
							try {
								await client.session.summarize({
									sessionID: agent.sessionId,
								})
							} catch (err) {
								log.error("session.summarize failed", { sessionId: agent.sessionId }, err)
							}
						}
					}
					return true
				case "fork":
					if (onForkFromTurn) {
						try {
							await onForkFromTurn()
						} catch (err) {
							log.error("slash /fork failed", { sessionId: agent.sessionId }, err)
						}
					}
					return true
				case "side": {
					if (!args) {
						slashCommandRef.current?.setText("/side ")
						return true
					}
					await onStartSideQuestion?.(args)
					return true
				}
				case "goal":
					setActiveTrigger("goal")
					return true
				case "plan":
					changeCollaborationMode("plan")
					return true
				case "skills":
					setSkillPickerOpen(true)
					return true
				case "research":
					return false
			}
		},
		[
			agent.directory,
			agent.sessionId,
			changeCollaborationMode,
			onForkFromTurn,
			onStartSideQuestion,
		],
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
			> = [{ type: "text", text: trigger === "goal" ? goalPromptText(text) : `/${trigger} ${text.trim()}` }]
			for (const file of files ?? []) {
				parts.push({
					type: "file",
					mime: file.mediaType ?? "application/octet-stream",
					filename: file.filename,
					url: file.url,
				})
			}
			// Flush any still-debounced composer selection so the turn starts
			// from exactly what the UI shows, then send only the explicit
			// selection — never a fallback-resolved model, which would
			// overwrite the persisted per-session choice.
			await flushSelectionPersist()
			await client.session.promptAsync({
				sessionID: agent.sessionId,
				parts,
				model: selectedModel
					? { providerID: selectedModel.providerID, modelID: selectedModel.modelID }
					: undefined,
				agent: selectedAgent || undefined,
				variant: selectedVariant,
			})
		},
		[
			agent.directory,
			agent.sessionId,
			flushSelectionPersist,
			selectedModel,
			selectedAgent,
			selectedVariant,
		],
	)

	const handleEditQueueItem = useCallback(
		async (item: ComposerQueueItem) => {
			try {
				const text = await editQueueItem(item)
				if (text) slashCommandRef.current?.setText(text)
			} catch (err) {
				log.error("edit queue item failed", { sessionId: agent.sessionId, queueItemId: item.id }, err)
			}
		},
		[agent.sessionId, editQueueItem],
	)

	const handleSend = useCallback(
		async (text: string, files?: FileAttachment[]) => {
			log.debug("handleSend called", {
				textLength: text.trim().length,
				hasOnSendMessage: !!onSendMessage,
				sending,
				sessionId: agent.sessionId,
			})
			if (recoveryState?.recovery || recoveryState?.pending || !text.trim() || (!onSendMessage && !activeTrigger) || sending) {
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
				// Only an explicit composer selection may become the project's
				// default model — a fallback-resolved model would poison the
				// preference (and then every future fallback) with request
				// slugs or defaults the user never chose.
				if (selectedModel && agent.directory) {
					appStore.set(setProjectModelAtom, {
						directory: agent.directory,
						model: {
							...selectedModel,
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
					// Land any still-debounced selection before the turn, and
					// send only the explicit selection — the server keeps the
					// session's persisted model otherwise.
					await flushSelectionPersist()
					await onSendMessage?.(agent, finalText, {
						model: selectedModel ?? undefined,
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
			recoveryState,
			onSendMessage,
			sending,
			agent,
			selectedModel,
			flushSelectionPersist,
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

	const queuePlaceholder = isWorking
		? queueItems.length > 0
			? "Add to queue…"
			: "Queue a follow-up…"
		: "What would you like to do?"

	const canSend = isConnected && !sending && !recoveryState?.recovery && !recoveryState?.pending

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
				changeCollaborationMode(collaborationMode === "plan" ? "build" : "plan")
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
		[handleEscapeAbort, handleSlashClose, handleMentionClose, changeCollaborationMode, collaborationMode],
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

					{effectiveQuestion ? (
						<ChatQuestionFlow
							questions={[effectiveQuestion.request]}
							isFromSubAgent={effectiveQuestion.sessionId !== agent.sessionId}
							onReply={handleReplyQuestion}
							onReject={handleRejectQuestion}
							disabled={!isConnected}
						/>
					) : effectivePermission ? (
						<ChatPermissionFlow
							agent={agent}
							permission={effectivePermission.request}
							onApprove={handleApprovePermission}
							onDeny={handleDenyPermission}
							disabled={!isConnected}
							isFromSubAgent={effectivePermission.sessionId !== agent.sessionId}
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
								<PromptInput
									className="devo-composer bg-background/95 shadow-[0_8px_32px_rgba(0,0,0,0.05)] dark:shadow-[0_10px_36px_rgba(0,0,0,0.28)]"
									accept="image/png,image/jpeg,image/gif,image/webp,application/pdf"
									multiple
									maxFileSize={10 * 1024 * 1024}
									onSubmit={(message) => {
										if (message.text.trim() && canSend)
											handleSend(message.text, message.files.length > 0 ? message.files : undefined)
									}}
								>
									{recoveryState && <TurnRecoveryPanel state={recoveryState} />}
                                    <ComposerStatusStack
										goal={activeGoal}
										goalAction={goalAction}
										queueItems={queueItems}
										draggingQueueItemId={draggingQueueItemId}
										onEditGoal={handleEditGoal}
										onPauseGoal={handlePauseGoal}
										onResumeGoal={handleResumeGoal}
										onClearGoal={handleClearGoal}
										onSteerQueueItem={steerQueueItem}
										onEditQueueItem={handleEditQueueItem}
										onRemoveQueueItem={removeQueueItem}
										onReorderQueueItem={reorderQueueItem}
										onQueueDragStart={setDraggingQueueItemId}
										onQueueDragEnd={() => setDraggingQueueItemId(null)}
									/>
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
										placeholder={queuePlaceholder}
									/>

									{/* Toolbar inside the card — agent + model + variant selectors + submit */}
									<PromptInputFooter>
										<PromptInputTools>
											<AttachButton disabled={!isConnected} />
											<ComposerPermissionPicker
												value={permissionProfile}
												onChange={handlePermissionProfileChange}
												disabled={!isConnected}
											/>
											{collaborationMode === "plan" && (
												<ComposerModeChip
													variant="plan"
													disabled={!isConnected}
													onRemove={() => changeCollaborationMode("build")}
												/>
											)}
											{activeTrigger === "goal" && (
												<ComposerModeChip
													variant="goal"
													disabled={!isConnected}
													onRemove={() => setActiveTrigger(null)}
												/>
											)}
										</PromptInputTools>
										<div className="ml-auto flex min-w-0 items-center gap-0.5">
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
											<PromptInputSubmit
												disabled={!canSend}
												status={isWorking ? "streaming" : undefined}
												onStop={handleStop}
											/>
										</div>
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
