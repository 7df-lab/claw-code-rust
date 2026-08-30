import { cn } from "@devo/ui/lib/utils"
import { useNavigate, useParams } from "@tanstack/react-router"
import { useAtom, useAtomValue } from "jotai"
import { ArrowLeftIcon } from "lucide-react"
import {
	type MutableRefObject,
	type RefObject,
	useCallback,
	useEffect,
	useLayoutEffect,
	useRef,
	useState,
} from "react"
import { sessionFamily } from "../atoms/sessions"
import { reviewPanelOpenAtom, reviewPanelSettingsAtom } from "../atoms/ui"
import { useSessionController } from "../hooks/use-session-controller"
import {
	evictMountedSession,
	initialMountedSessionIds,
	updateMountedSessionIds,
} from "../hooks/use-session-keep-alive-logic"
import { SessionPanelHeader } from "./agent-detail"
import { ChatInputSection, ChatView, type ChatScrollHandle } from "./chat/chat-view"
import { ReviewPanel } from "./review/review-panel"

interface SessionShellProps {
	activeSessionId: string | null
	evictRef?: MutableRefObject<(sessionId: string) => void>
}

function SessionTranscriptPanel({
	sessionId,
	isActive,
	scrollRef,
	composerInsetPx,
	reviewPanelOpen,
	sideQuestionHandlerRef,
}: {
	sessionId: string
	isActive: boolean
	scrollRef: RefObject<ChatScrollHandle | null>
	composerInsetPx: number
	reviewPanelOpen: boolean
	sideQuestionHandlerRef: MutableRefObject<((question: string) => Promise<void>) | null>
}) {
	const controller = useSessionController(sessionId, isActive)
	const { agent } = controller

	if (!agent) {
		if (!isActive) return null
		if (controller.resolving) {
			return (
				<div className="flex h-full items-center justify-center">
					<div className="size-4 animate-spin rounded-full border-2 border-muted-foreground/20 border-t-muted-foreground/60" />
				</div>
			)
		}
		return (
			<div className="flex h-full items-center justify-center">
				<div className="text-center">
					<p className="text-sm font-medium text-muted-foreground">Session not found</p>
					<p className="mt-1 text-xs text-muted-foreground/60">
						This session may have been deleted or is not yet loaded
					</p>
				</div>
			</div>
		)
	}

	return (
		<ChatView
			turns={controller.chatTurns}
			loading={controller.chatLoading ?? false}
			showLoading={controller.chatShowLoading ?? false}
			loadingEarlier={controller.chatLoadingEarlier ?? false}
			hasEarlierMessages={controller.chatHasEarlier ?? false}
			onLoadEarlier={controller.chatLoadEarlier}
			agent={agent}
			isConnected
			onSendMessage={controller.handleSendMessage}
			onStop={controller.handleStopAgent}
			providers={controller.providers}
			config={controller.config}
			vcs={controller.vcs}
			devoAgents={controller.devoAgents}
			onApprove={controller.handleApprovePermission}
			onDeny={controller.handleDenyPermission}
			onReplyQuestion={controller.handleReplyQuestion}
			onRejectQuestion={controller.handleRejectQuestion}
			canUndo={controller.canUndo}
			canRedo={controller.canRedo}
			onUndo={controller.undo}
			onRedo={controller.redo}
			isReverted={controller.isReverted}
			onForkFromTurn={controller.handleForkFromTurn}
			onEditUserMessage={controller.handleEditUserMessage}
			onDeletePart={controller.handleDeletePart}
			parentSessionName={controller.parentSessionName}
			reviewPanelOpen={reviewPanelOpen}
			isActive={isActive}
			showComposer={false}
			showTranscript
			externalScrollRef={isActive ? scrollRef : undefined}
			composerInsetPx={composerInsetPx}
			sideQuestionHandlerRef={sideQuestionHandlerRef}
		/>
	)
}

export function SessionShell({ activeSessionId, evictRef }: SessionShellProps) {
	const [mountedSessionIds, setMountedSessionIds] = useState<string[]>(() =>
		initialMountedSessionIds(activeSessionId),
	)
	const evictSession = useCallback((sessionId: string) => {
		setMountedSessionIds((prev) => evictMountedSession(prev, sessionId))
	}, [])

	useEffect(() => {
		if (!activeSessionId) return
		setMountedSessionIds((prev) => updateMountedSessionIds(prev, activeSessionId))
	}, [activeSessionId])
	const navigate = useNavigate()
	const { projectSlug } = useParams({ strict: false }) as { projectSlug?: string }
	const scrollRef = useRef<ChatScrollHandle | null>(null)
	const sideQuestionHandlerRef = useRef<((question: string) => Promise<void>) | null>(null)
	const composerRef = useRef<HTMLDivElement | null>(null)
	const [composerInset, setComposerInset] = useState(0)
	const [reviewPanelOpen, setReviewPanelOpen] = useAtom(reviewPanelOpenAtom)
	const [reviewSettings, setReviewSettings] = useAtom(reviewPanelSettingsAtom)

	useEffect(() => {
		const handleKeyDown = (event: KeyboardEvent) => {
			if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key === "d") {
				event.preventDefault()
				setReviewPanelOpen((prev) => !prev)
			}
			if ((event.metaKey || event.ctrlKey) && event.shiftKey && event.key === "f") {
				event.preventDefault()
				if (reviewPanelOpen) {
					setReviewSettings((prev) => ({ ...prev, expanded: !prev.expanded }))
				}
			}
		}
		document.addEventListener("keydown", handleKeyDown)
		return () => document.removeEventListener("keydown", handleKeyDown)
	}, [reviewPanelOpen, setReviewPanelOpen, setReviewSettings])

	const activeController = useSessionController(activeSessionId ?? "", true)
	const activeAgent = activeController.agent
	const isWorking = activeAgent?.status === "running"
	const sessionEntry = useAtomValue(sessionFamily(activeSessionId ?? ""))
	const setupPhase = sessionEntry?.setupPhase

	const [isEditingTitle, setIsEditingTitle] = useState(false)
	const [titleValue, setTitleValue] = useState(activeAgent?.name ?? "")
	const titleInputRef = useRef<HTMLInputElement>(null)

	useEffect(() => {
		if (!evictRef) return
		evictRef.current = evictSession
	}, [evictRef, evictSession])

	useEffect(() => {
		if (activeAgent) {
			setTitleValue(activeAgent.name)
		}
	}, [activeAgent?.name, activeAgent?.sessionId])

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
			window.addEventListener("resize", updateComposerInset)
			return () => window.removeEventListener("resize", updateComposerInset)
		}

		const resizeObserver = new ResizeObserver(updateComposerInset)
		resizeObserver.observe(composer)
		return () => resizeObserver.disconnect()
	}, [setupPhase, activeSessionId])

	if (!activeSessionId || mountedSessionIds.length === 0) {
		return null
	}

	return (
		<div className="flex h-full">
			<div className="flex min-w-0 flex-1 flex-col">
				{activeAgent ? (
					<SessionPanelHeader
						agent={activeAgent}
						isEditingTitle={isEditingTitle}
						titleValue={titleValue}
						titleInputRef={titleInputRef}
						onTitleValueChange={setTitleValue}
						onStartEditing={() => {
							if (!activeController.handleRenameSession) return
							setTitleValue(activeAgent.name)
							setIsEditingTitle(true)
						}}
						onConfirmTitle={async () => {
							const trimmed = titleValue.trim()
							setIsEditingTitle(false)
							if (trimmed && trimmed !== activeAgent.name) {
								await activeController.handleRenameSession(activeAgent, trimmed)
							}
						}}
						onCancelEditing={() => {
							setIsEditingTitle(false)
							setTitleValue(activeAgent.name)
						}}
						onRename={activeController.handleRenameSession}
						reviewPanelOpen={reviewPanelOpen}
						onToggleReviewPanel={() => setReviewPanelOpen((prev) => !prev)}
					/>
				) : activeController.resolving ? (
					<div className="flex h-[44px] shrink-0 items-center justify-center border-b border-border/40">
						<div className="size-3 animate-spin rounded-full border-2 border-muted-foreground/20 border-t-muted-foreground/60" />
					</div>
				) : null}

				{activeAgent?.parentId && !activeAgent.forkFromId ? (
					<button
						type="button"
						onClick={() => {
							if (!activeAgent.parentId) return
							navigate({
								to: "/project/$projectSlug/session/$sessionId",
								params: {
									projectSlug: projectSlug ?? activeAgent.projectSlug,
									sessionId: activeAgent.parentId,
								},
							})
						}}
						className="flex items-center gap-1.5 border-b border-border bg-muted/30 px-4 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-muted/50 hover:text-foreground"
					>
						<ArrowLeftIcon className="size-3" />
						<span>
							Back to{" "}
							<span className="font-medium text-foreground">
								{activeController.parentSessionName || "parent session"}
							</span>
						</span>
					</button>
				) : null}

				<div className="relative min-h-0 flex-1">
					{mountedSessionIds.map((id) => {
						const isActive = id === activeSessionId
						return (
							<div
								key={id}
								className={cn(
									"absolute inset-0 h-full transition-opacity duration-150",
									isActive ? "opacity-100" : "pointer-events-none invisible opacity-0",
								)}
								aria-hidden={!isActive}
							>
								<SessionTranscriptPanel
									sessionId={id}
									isActive={isActive}
									scrollRef={scrollRef}
									composerInsetPx={composerInset}
									reviewPanelOpen={reviewPanelOpen}
									sideQuestionHandlerRef={sideQuestionHandlerRef}
								/>
							</div>
						)
					})}
				</div>

				{activeAgent && !setupPhase ? (
					<div ref={composerRef} className="relative z-30 shrink-0 px-3.5 pb-3 pt-1">
						<ChatInputSection
							agent={activeAgent}
							turns={activeController.chatTurns}
							isConnected
							isWorking={isWorking}
							onSendMessage={activeController.handleSendMessage}
							onStop={activeController.handleStopAgent}
							providers={activeController.providers}
							config={activeController.config}
							devoAgents={activeController.devoAgents}
							onApprove={activeController.handleApprovePermission}
							onDeny={activeController.handleDenyPermission}
							onReplyQuestion={activeController.handleReplyQuestion}
							onRejectQuestion={activeController.handleRejectQuestion}
							onForkFromTurn={activeController.handleForkFromTurn}
							onStartSideQuestion={async (question) => {
								await sideQuestionHandlerRef.current?.(question)
							}}
							canRedo={activeController.canRedo}
							onRedo={activeController.redo}
							isReverted={activeController.isReverted}
							scrollRef={scrollRef}
							reviewPanelOpen={reviewPanelOpen}
						/>
					</div>
				) : null}
			</div>

			<div
				className="shrink-0 overflow-hidden border-l border-border transition-[width] duration-250 ease-in-out"
				style={{ width: reviewPanelOpen ? (reviewSettings.expanded ? "100%" : "40%") : 0 }}
			>
				{activeSessionId ? (
					<div
						className="h-full"
						style={{ minWidth: reviewSettings.expanded ? "100vw" : "40vw" }}
					>
						<ReviewPanel
							sessionId={activeSessionId}
							directory={activeAgent?.directory ?? ""}
						/>
					</div>
				) : null}
			</div>
		</div>
	)
}
