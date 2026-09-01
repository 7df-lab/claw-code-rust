import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuTrigger,
} from "@devo/ui/components/dropdown-menu"
import { Input } from "@devo/ui/components/input"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { cn } from "@devo/ui/lib/utils"
import { useNavigate, useParams } from "@tanstack/react-router"
import { useAtom, useAtomValue } from "jotai"
import {
	ArrowLeftIcon,
	CheckIcon,
	ChevronDownIcon,
	ExternalLinkIcon,
	GitForkIcon,
	PencilIcon,
} from "lucide-react"
import { useCallback, useEffect, useRef, useState } from "react"
import type { OpenInTarget } from "../../preload/api"
import { terminalPanelOpenAtom } from "../atoms/terminal"
import { reviewPanelOpenAtom, reviewPanelSettingsAtom } from "../atoms/ui"
import {
	latestWorkspaceTurnIdFamily,
	workspaceChangesKey,
	workspaceChangesStateFamily,
} from "../atoms/workspace-changes"
import type {
	ConfigData,
	ModelRef,
	ProvidersData,
	SdkAgent,
	VcsData,
} from "../hooks/use-devo-data"
import type { ChatTurn } from "../hooks/use-session-chat"
import { formatShortcut } from "../lib/shortcut-display"
import type { Agent, FileAttachment, PermissionResponse, QuestionAnswer } from "../lib/types"
import { workspaceChangeStats } from "../lib/workspace-diff"
import {
	fetchOpenInTargets,
	isElectron,
	openInTarget,
	setOpenInPreferred,
} from "../services/backend"
import { ChatView } from "./chat"
import { ContextUsageButton } from "./context-usage-button"
import { BottomPanelIcon, RightPanelIcon } from "./panel-icons"
import { ReviewPanel } from "./review/review-panel"
import { WorktreeActions } from "./worktree-actions"

function useTurnWorkspaceChangeStats(sessionId: string): {
	fileCount: number
	additions: number
	deletions: number
} {
	const latestTurnId = useAtomValue(latestWorkspaceTurnIdFamily(sessionId))
	const key = workspaceChangesKey({
		sessionId,
		scope: "turn",
		turnId: latestTurnId,
	})
	const state = useAtomValue(workspaceChangesStateFamily(key))
	if (state.view) return workspaceChangeStats(state.view)
	if (state.summary) {
		return {
			fileCount: state.summary.stats.files_changed,
			additions: state.summary.stats.additions,
			deletions: state.summary.stats.deletions,
		}
	}
	return { fileCount: 0, additions: 0, deletions: 0 }
}

interface AgentDetailProps {
	agent: Agent
	/** Structured chat turns (for Chat tab) */
	chatTurns: ChatTurn[]
	chatLoading?: boolean
	/** True when the initial fetch is in flight and no cached turns exist yet. */
	chatShowLoading?: boolean
	/** Whether earlier messages are currently being loaded */
	chatLoadingEarlier?: boolean
	/** Whether there are earlier messages that can be loaded */
	chatHasEarlier?: boolean
	/** Callback to load earlier messages */
	onLoadEarlier?: () => void
	onStop?: (agent: Agent) => Promise<void>
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
	onReplyQuestion?: (agent: Agent, requestId: string, answers: QuestionAnswer[]) => Promise<void>
	onRejectQuestion?: (agent: Agent, requestId: string) => Promise<void>
	onSendMessage?: (
		agent: Agent,
		message: string,
		options?: { model?: ModelRef; agentName?: string; variant?: string; files?: FileAttachment[] },
	) => Promise<void>
	onRename?: (agent: Agent, title: string) => Promise<void>
	/** Display name of the parent session (for breadcrumb) */
	parentSessionName?: string
	isConnected?: boolean
	/** Provider data for model selector */
	providers?: ProvidersData | null
	/** Config data (default model, default agent) */
	config?: ConfigData | null
	/** VCS data for status bar */
	vcs?: VcsData | null
	/** Available Devo agents for agent selector */
	devoAgents?: SdkAgent[]
	/** Whether undo is available */
	canUndo?: boolean
	/** Whether redo is available */
	canRedo?: boolean
	/** Undo handler — returns the undone user message text */
	onUndo?: () => Promise<string | undefined>
	/** Redo handler */
	onRedo?: () => Promise<void>
	/** Whether the session is in a reverted state */
	isReverted?: boolean
	/** Fork from a turn boundary (protocol turn id, or undefined for tip fork) */
	onForkFromTurn?: (turnId?: string) => Promise<void>
	/** Edit and resend the latest user message */
	onEditUserMessage?: (messageId: string, text: string) => Promise<void>
	/** Delete a specific part from a message (for error recovery) */
	onDeletePart?: (sessionId: string, messageId: string, partId: string) => Promise<void>
}

export function AgentDetail({
	agent,
	chatTurns,
	chatLoading,
	chatShowLoading,
	onStop,
	onApprove,
	onDeny,
	onReplyQuestion,
	onRejectQuestion,
	onSendMessage,
	onRename,
	parentSessionName,
	isConnected,
	providers,
	config,
	vcs,
	devoAgents,
	chatLoadingEarlier,
	chatHasEarlier,
	onLoadEarlier,
	canUndo,
	canRedo,
	onUndo,
	onRedo,
	isReverted,
	onForkFromTurn,
	onEditUserMessage,
	onDeletePart,
}: AgentDetailProps) {
	const navigate = useNavigate()
	const { projectSlug } = useParams({ strict: false }) as { projectSlug?: string }

	const [isEditingTitle, setIsEditingTitle] = useState(false)
	const [titleValue, setTitleValue] = useState(agent.name)
	const titleInputRef = useRef<HTMLInputElement>(null)

	// Review panel state
	const [reviewPanelOpen, setReviewPanelOpen] = useAtom(reviewPanelOpenAtom)
	const [reviewSettings, setReviewSettings] = useAtom(reviewPanelSettingsAtom)

	// Keyboard shortcut: Cmd/Ctrl+Shift+D to toggle review panel
	useEffect(() => {
		const handleKeyDown = (e: KeyboardEvent) => {
			if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "d") {
				e.preventDefault()
				setReviewPanelOpen((prev) => !prev)
			}
			if ((e.metaKey || e.ctrlKey) && e.shiftKey && e.key === "f") {
				e.preventDefault()
				if (reviewPanelOpen) {
					setReviewSettings((prev) => ({ ...prev, expanded: !prev.expanded }))
				}
			}
		}
		document.addEventListener("keydown", handleKeyDown)
		return () => document.removeEventListener("keydown", handleKeyDown)
	}, [setReviewPanelOpen, setReviewSettings, reviewPanelOpen])

	// Close review panel when navigating to a session with no diffs
	const prevSessionIdRef = useRef(agent.sessionId)
	const diffStats = useTurnWorkspaceChangeStats(agent.sessionId)
	useEffect(() => {
		if (prevSessionIdRef.current !== agent.sessionId) {
			prevSessionIdRef.current = agent.sessionId
			if (diffStats.fileCount === 0) {
				setReviewPanelOpen(false)
			}
		}
	}, [agent.sessionId, diffStats.fileCount, setReviewPanelOpen])

	const startEditingTitle = useCallback(() => {
		if (!onRename) return
		setTitleValue(agent.name)
		setIsEditingTitle(true)
	}, [agent.name, onRename])

	const confirmTitle = useCallback(async () => {
		const trimmed = titleValue.trim()
		setIsEditingTitle(false)
		if (trimmed && trimmed !== agent.name && onRename) {
			await onRename(agent, trimmed)
		}
	}, [titleValue, agent, onRename])

	const cancelEditingTitle = useCallback(() => {
		setIsEditingTitle(false)
		setTitleValue(agent.name)
	}, [agent.name])

	useEffect(() => {
		if (isEditingTitle && titleInputRef.current) {
			titleInputRef.current.focus()
			titleInputRef.current.select()
		}
	}, [isEditingTitle])

	const chatContent = (
		<>
			<SessionPanelHeader
				agent={agent}
				isEditingTitle={isEditingTitle}
				titleValue={titleValue}
				titleInputRef={titleInputRef}
				onTitleValueChange={setTitleValue}
				onStartEditing={startEditingTitle}
				onConfirmTitle={confirmTitle}
				onCancelEditing={cancelEditingTitle}
				onRename={onRename}
				reviewPanelOpen={reviewPanelOpen}
				onToggleReviewPanel={() => setReviewPanelOpen((prev) => !prev)}
			/>

			{/* Sub-agent breadcrumb — navigate back to parent (forks use the in-transcript marker) */}
			{agent.parentId && !agent.forkFromId && (
				<button
					type="button"
					onClick={() => {
						if (!agent.parentId) return
						navigate({
							to: "/project/$projectSlug/session/$sessionId",
							params: { projectSlug: projectSlug ?? agent.projectSlug, sessionId: agent.parentId },
						})
					}}
					className="flex items-center gap-1.5 border-b border-border bg-muted/30 px-4 py-1.5 text-xs text-muted-foreground transition-colors hover:bg-muted/50 hover:text-foreground"
				>
					<ArrowLeftIcon className="size-3" />
					<span>
						Back to{" "}
						<span className="font-medium text-foreground">
							{parentSessionName || "parent session"}
						</span>
					</span>
				</button>
			)}

			{/* Chat -- full height */}
			<div className="min-h-0 flex-1">
				<ChatView
					turns={chatTurns}
					loading={chatLoading ?? false}
					showLoading={chatShowLoading ?? false}
					loadingEarlier={chatLoadingEarlier ?? false}
					hasEarlierMessages={chatHasEarlier ?? false}
					onLoadEarlier={onLoadEarlier}
					agent={agent}
					isConnected={isConnected ?? false}
					onSendMessage={onSendMessage}
					onStop={onStop}
					providers={providers}
					config={config}
					vcs={vcs}
					devoAgents={devoAgents}
					onApprove={onApprove}
					onDeny={onDeny}
					onReplyQuestion={onReplyQuestion}
					onRejectQuestion={onRejectQuestion}
					canUndo={canUndo}
					canRedo={canRedo}
					onUndo={onUndo}
					onRedo={onRedo}
					isReverted={isReverted}
					onForkFromTurn={onForkFromTurn}
					onEditUserMessage={onEditUserMessage}
					onDeletePart={onDeletePart}
					parentSessionName={parentSessionName}
					reviewPanelOpen={reviewPanelOpen}
				/>
			</div>
		</>
	)

	return (
		<div className="flex h-full">
			{/* Chat panel -- takes remaining space */}
			<div className="min-w-0 flex-1 flex flex-col">{chatContent}</div>

			{/* Review panel -- slides in/out from right */}
			<div
				className="shrink-0 overflow-hidden border-l border-border transition-[width] duration-250 ease-in-out"
				style={{ width: reviewPanelOpen ? (reviewSettings.expanded ? "100%" : "40%") : 0 }}
			>
				{/* Keep ReviewPanel mounted so it retains state, just hidden at 0 width */}
				<div className="h-full" style={{ minWidth: reviewSettings.expanded ? "100vw" : "40vw" }}>
					<ReviewPanel sessionId={agent.sessionId} directory={agent.directory} />
				</div>
			</div>
		</div>
	)
}

// ============================================================
// Session panel header
// ============================================================

export function SessionPanelHeader({
	agent,
	isEditingTitle,
	titleValue,
	titleInputRef,
	onTitleValueChange,
	onStartEditing,
	onConfirmTitle,
	onCancelEditing,
	onRename,
	reviewPanelOpen,
	onToggleReviewPanel,
}: {
	agent: Agent
	isEditingTitle: boolean
	titleValue: string
	titleInputRef: React.RefObject<HTMLInputElement | null>
	onTitleValueChange: (v: string) => void
	onStartEditing: () => void
	onConfirmTitle: () => void
	onCancelEditing: () => void
	onRename?: (agent: Agent, title: string) => Promise<void>
	reviewPanelOpen: boolean
	onToggleReviewPanel: () => void
}) {
	const diffStats = useTurnWorkspaceChangeStats(agent.sessionId)

	return (
		<div
			data-slot="session-panel-header"
			className="flex h-[44px] w-full min-w-0 shrink-0 items-center gap-2.5 border-b border-border/40 px-5"
		>
			{/* Session title (+ optional worktree branch badge) */}
			<div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-hidden">
				{agent.worktreeBranch && <WorktreeBranchBadge branch={agent.worktreeBranch} />}

				{/* Session name — click to edit */}
				{isEditingTitle ? (
					<div className="inline-grid min-w-0 max-w-full flex-1 items-center">
						{/* Ghost span — sizes the grid column to match the text width */}
						<span className="invisible col-start-1 row-start-1 truncate text-[13px] font-medium leading-none">
							{titleValue}
						</span>
						<Input
							ref={titleInputRef}
							value={titleValue}
							onChange={(e) => onTitleValueChange(e.target.value)}
							onKeyDown={(e) => {
								e.stopPropagation()
								if (e.key === "Enter") onConfirmTitle()
								if (e.key === "Escape") onCancelEditing()
							}}
							onBlur={onConfirmTitle}
							className="col-start-1 row-start-1 h-7 min-w-0 border-none bg-transparent p-0 text-[13px] md:text-[13px] font-medium leading-none shadow-none focus-visible:ring-0"
						/>
					</div>
				) : (
					<button
						type="button"
						onClick={onRename ? onStartEditing : undefined}
						className={`group flex min-w-0 items-center gap-1.5 ${onRename ? "cursor-pointer" : "cursor-default"}`}
					>
						<h2 className="min-w-0 truncate text-[13px] font-medium leading-none">
							{agent.name}
						</h2>
						{onRename && (
							<PencilIcon className="size-3 shrink-0 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100" />
						)}
					</button>
				)}
			</div>

				{/* Right-aligned items */}
				<div className="flex min-w-0 shrink-0 items-center gap-2.5 overflow-hidden">
					{/* Worktree actions (Apply to local, Commit & push) */}
					{agent.worktreePath && <WorktreeActions agent={agent} />}

					{agent.worktreePath && <div className="hidden h-3 w-px shrink-0 bg-border/60 md:block" />}

					{/* Open in external editor */}
					<div className="hidden md:block">
						<OpenInButton directory={agent.worktreePath ?? agent.directory} />
					</div>

					<div className="flex shrink-0 items-center gap-0.5">
						<ContextUsageButton sessionId={agent.sessionId} />
						<TerminalToggleButton />
						<ChangesPanelToggleButton
							reviewPanelOpen={reviewPanelOpen}
							diffStats={diffStats}
							onToggleReviewPanel={onToggleReviewPanel}
						/>
					</div>
				</div>
			</div>
	)
}

// ============================================================
// Open in external editor/terminal
// ============================================================

// OpenInTarget type is provided by fetchOpenInTargets() via OpenInTargetsResult

/**
 * Renders a small app icon for a target. Uses runtime-resolved icon data URLs
 * from Electron's app.getFileIcon() API. Falls back to ExternalLinkIcon
 * if no icon data is available.
 */
function TargetIcon({ iconDataUrl, className }: { iconDataUrl?: string; className?: string }) {
	if (!iconDataUrl) return <ExternalLinkIcon className={className} />
	return (
		<img
			alt=""
			aria-hidden="true"
			src={iconDataUrl}
			className={cn("shrink-0 object-contain", className)}
		/>
	)
}

/**
 * Dropdown button that opens the project directory in an available editor,
 * terminal, or file manager. Fetches targets lazily on first open.
 *
 * The primary action (clicking the main button) opens in the preferred target.
 * The chevron opens a dropdown to choose a different target.
 */
function OpenInButton({ directory }: { directory: string }) {
	const [targets, setTargets] = useState<OpenInTarget[]>([])
	const [preferred, setPreferred] = useState<string | null>(null)
	const [loaded, setLoaded] = useState(false)
	const [opening, setOpening] = useState<string | null>(null)

	const loadTargets = useCallback(async () => {
		if (loaded) {
			return { targets, preferredTarget: preferred }
		}
		try {
			const result = await fetchOpenInTargets()
			const availableTargets = result.targets.filter((t) => t.available)
			setTargets(availableTargets)
			setPreferred(result.preferredTarget)
			setLoaded(true)
			return { targets: availableTargets, preferredTarget: result.preferredTarget }
		} catch {
			// Silently fail — button will show no targets
			setLoaded(true)
			return { targets: [], preferredTarget: null }
		}
	}, [loaded, preferred, targets])

	useEffect(() => {
		void loadTargets()
	}, [loadTargets])

	const handleOpen = useCallback(
		async (targetId: string) => {
			setOpening(targetId)
			try {
				await openInTarget(directory, targetId, true)
				setPreferred(targetId)
			} catch {
				// Silently fail
			} finally {
				setOpening(null)
			}
		},
		[directory],
	)

	const handleSelectTarget = useCallback(
		async (targetId: string) => {
			const previousTarget = preferred
			setPreferred(targetId)
			try {
				await setOpenInPreferred(targetId)
			} catch {
				setPreferred(previousTarget)
			}
		},
		[preferred],
	)

	const handlePrimaryClick = useCallback(async () => {
		const { targets: availableTargets, preferredTarget } = loaded
			? { targets, preferredTarget: preferred }
			: await loadTargets()
		const target = preferredTarget
			? availableTargets.find((t) => t.id === preferredTarget)
			: availableTargets[0]
		if (target) {
			await handleOpen(target.id)
		}
	}, [loaded, loadTargets, preferred, targets, handleOpen])

	// Don't show on non-Electron
	if (!isElectron) return null

	// Resolve the preferred target's icon data URL for the primary button
	const preferredTarget = targets.find((t) => t.id === preferred)

	return (
		<div className="flex items-center rounded-md border border-border/80 ring-1 ring-border/25">
			<button
				type="button"
				onClick={handlePrimaryClick}
				disabled={opening !== null}
				className="flex items-center gap-1.5 rounded-l-md px-2 py-1 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:opacity-50"
			>
				{preferredTarget?.iconDataUrl ? (
					<TargetIcon iconDataUrl={preferredTarget.iconDataUrl} className="size-3.5" />
				) : (
					<ExternalLinkIcon className="size-3" />
				)}
				<span>Open in</span>
			</button>

			<DropdownMenu onOpenChange={(open) => open && loadTargets()}>
				<DropdownMenuTrigger
					render={
						<button
							type="button"
							className="rounded-r-md border-l border-border/70 px-1 py-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
						/>
					}
				>
					<ChevronDownIcon className="size-3" />
				</DropdownMenuTrigger>
				<DropdownMenuContent align="end" className="min-w-[180px]">
					{!loaded ? (
						<DropdownMenuItem disabled>Loading...</DropdownMenuItem>
					) : targets.length === 0 ? (
						<DropdownMenuItem disabled>No editors found</DropdownMenuItem>
					) : (
						targets.map((target) => (
							<DropdownMenuItem
								key={target.id}
								onClick={() => handleSelectTarget(target.id)}
								disabled={opening !== null}
								className="flex items-center gap-2"
							>
								<TargetIcon iconDataUrl={target.iconDataUrl} className="size-4" />
								<span className="flex-1">{target.label}</span>
								{preferred === target.id && (
									<CheckIcon className="size-3 shrink-0 text-muted-foreground/60" />
								)}
							</DropdownMenuItem>
						))
					)}
				</DropdownMenuContent>
			</DropdownMenu>
		</div>
	)
}

/**
 * Compact badge showing the worktree branch name with a copy action.
 */
function WorktreeBranchBadge({ branch }: { branch: string }) {
	const [copied, setCopied] = useState(false)

	const handleCopy = useCallback(async () => {
		await navigator.clipboard.writeText(branch)
		setCopied(true)
		setTimeout(() => setCopied(false), 2000)
	}, [branch])

	return (
		<Tooltip>
			<TooltipTrigger
				render={
					<button
						type="button"
						onClick={handleCopy}
						className="flex shrink-0 items-center gap-1 rounded-md bg-muted/60 px-1.5 py-0.5 text-[10px] leading-none text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
					/>
				}
			>
				<GitForkIcon className="size-2.5" aria-hidden="true" />
				<span className="max-w-[120px] truncate">{branch}</span>
				{copied && <CheckIcon className="size-2.5 text-green-500" />}
			</TooltipTrigger>
			<TooltipContent>Click to copy branch name</TooltipContent>
		</Tooltip>
	)
}

function TerminalToggleButton() {
	const [terminalPanelOpen, setTerminalPanelOpen] = useAtom(terminalPanelOpenAtom)
	const toggleTerminalShortcut = formatShortcut(["mod", "J"])

	return (
		<Tooltip>
			<TooltipTrigger
				aria-label="Toggle terminal"
				render={
					<button
						type="button"
						onClick={() => setTerminalPanelOpen((open) => !open)}
						className={cn(
							"inline-flex size-8 items-center justify-center rounded-md transition-colors",
							terminalPanelOpen
								? "bg-muted text-foreground"
								: "text-muted-foreground hover:bg-muted hover:text-foreground",
						)}
					/>
				}
			>
				<BottomPanelIcon
					open={terminalPanelOpen}
					className="size-4"
					aria-hidden="true"
				/>
			</TooltipTrigger>
			<TooltipContent>Toggle terminal ({toggleTerminalShortcut})</TooltipContent>
		</Tooltip>
	)
}

function ChangesPanelToggleButton({
	reviewPanelOpen,
	diffStats,
	onToggleReviewPanel,
}: {
	reviewPanelOpen: boolean
	diffStats: { fileCount: number; additions: number; deletions: number }
	onToggleReviewPanel: () => void
}) {
	const toggleReviewPanelShortcut = formatShortcut(["shift", "mod", "D"])
	const statsLabel =
		diffStats.fileCount > 0
			? `, ${diffStats.fileCount} files, +${diffStats.additions} -${diffStats.deletions}`
			: ""

	return (
		<Tooltip>
			<TooltipTrigger
				aria-label="Toggle changes panel"
				render={
					<button
						type="button"
						onClick={onToggleReviewPanel}
						className={cn(
							"inline-flex size-8 items-center justify-center rounded-md transition-colors",
							reviewPanelOpen
								? "bg-muted text-foreground"
								: "text-muted-foreground hover:bg-muted hover:text-foreground",
						)}
					/>
				}
			>
				<RightPanelIcon open={reviewPanelOpen} className="size-4" aria-hidden="true" />
			</TooltipTrigger>
			<TooltipContent>
				{`${reviewPanelOpen ? "Hide changes panel" : "Show changes panel"}${statsLabel} (${toggleReviewPanelShortcut})`}
			</TooltipContent>
		</Tooltip>
	)
}
