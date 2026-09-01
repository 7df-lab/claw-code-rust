import {
	ContextMenu,
	ContextMenuContent,
	ContextMenuItem,
	ContextMenuSeparator,
	ContextMenuTrigger,
} from "@devo/ui/components/context-menu"
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuGroup,
	DropdownMenuItem,
	DropdownMenuSeparator,
	DropdownMenuTrigger,
} from "@devo/ui/components/dropdown-menu"
import {
	optionMenuIconClass,
	optionMenuSeparatorClass,
} from "@devo/ui/components/option-menu-styles"
import { cn } from "@devo/ui/lib/utils"
import { useNavigate } from "@tanstack/react-router"
import {
	ArchiveIcon,
	ArrowUpRightIcon,
	ChevronDownIcon,
	ChevronRightIcon,
	FolderClosedIcon,
	FolderOpenIcon,
	GitForkIcon,
	Loader2Icon,
	MoreHorizontalIcon,
	PencilIcon,
	PenLineIcon,
	PinIcon,
	SplitIcon,
	TrashIcon,
} from "lucide-react"
import {
	Fragment,
	memo,
	useCallback,
	useEffect,
	useRef,
	useState,
	useTransition,
	type MouseEvent,
	type ReactNode,
} from "react"
import { formatRelativeTime } from "../../atoms/derived/agents"
import { appStore } from "../../atoms/store"
import { customizeOpenAtom } from "../../atoms/ui"
import type { Agent, AgentStatus, SidebarProject } from "../../lib/types"
import {
	buildProjectRowActions,
	buildSessionRowActions,
	type ProjectRowActionId,
	type SessionRowActionId,
	type SidebarRowAction,
} from "./sidebar-row-actions"
import {
	projectMenuContentClass,
	rowMenuItemClass,
	sessionMenuContentClass,
} from "./sidebar-menu-styles"

function useLiveLastActive(lastActiveAt: number): string {
	const [display, setDisplay] = useState(() => formatRelativeTime(lastActiveAt))

	useEffect(() => {
		setDisplay(formatRelativeTime(lastActiveAt))
		const id = setInterval(() => setDisplay(formatRelativeTime(lastActiveAt)), 60_000)
		return () => clearInterval(id)
	}, [lastActiveAt])

	return display
}

function statusIndicatorClass(status: AgentStatus): string {
	if (status === "failed") return "bg-destructive"
	if (status === "running" || status === "waiting") return "bg-[#3396f4]"
	return "bg-transparent"
}

function SessionRowStatusIndicator({
	agent,
	lastActive,
	isWorktree,
}: {
	agent: Agent
	lastActive: string
	isWorktree: boolean
}) {
	if (agent.status === "waiting") {
		return (
			<>
				<span className={cn("size-2 rounded-full", statusIndicatorClass(agent.status))} />
				{isWorktree && <GitForkIcon className="size-3.5 text-muted-foreground" />}
			</>
		)
	}
	if (agent.status === "failed") {
		return (
			<>
				<span className={cn("size-2 rounded-full", statusIndicatorClass(agent.status))} />
				{isWorktree && <GitForkIcon className="size-3.5 text-muted-foreground" />}
			</>
		)
	}
	return (
		<>
			{lastActive}
			{isWorktree && <GitForkIcon className="size-3.5 text-muted-foreground" />}
		</>
	)
}

const rowMenuIconClass = optionMenuIconClass
const sidebarPrimaryIconClass = "size-[15px] stroke-[1.5]"
const sessionTitleTextClass = "text-[13px] font-normal leading-tight tracking-normal"
const sessionStatusHiddenClass =
	"transition-opacity duration-150 group-hover/sidebar-row:opacity-0 group-focus-within/sidebar-row:opacity-0 group-has-[[data-popup-open]]/sidebar-row:opacity-0"
const sessionActionsVisibleClass =
	"group-hover/sidebar-row:opacity-100 group-focus-within/sidebar-row:opacity-100 group-has-[[data-popup-open]]/sidebar-row:opacity-100"
const rowActionButtonBaseClass =
	"flex items-center justify-center rounded-lg bg-transparent text-muted-foreground opacity-0 transition-[color,opacity] hover:text-sidebar-foreground focus-visible:text-sidebar-foreground focus-visible:opacity-100 focus-visible:outline-none data-popup-open:opacity-100"
const sessionRowActionButtonClass = cn(
	"absolute inset-0 pointer-events-none group-hover/sidebar-row:pointer-events-auto group-focus-within/sidebar-row:pointer-events-auto group-has-[[data-popup-open]]/sidebar-row:pointer-events-auto focus-visible:pointer-events-auto",
	rowActionButtonBaseClass,
	sessionActionsVisibleClass,
)
const sessionStatusContainerClass =
	"pointer-events-auto absolute right-2 top-1/2 flex min-h-7 min-w-7 -translate-y-1/2 items-center justify-center gap-1.5 rounded-lg px-1 text-[13px] tabular-nums text-muted-foreground"
const inlineRowActionButtonClass =
	"flex size-6 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-black/[0.06] hover:text-sidebar-foreground focus-visible:bg-black/[0.06] focus-visible:text-sidebar-foreground focus-visible:outline-none dark:hover:bg-white/[0.08] dark:focus-visible:bg-white/[0.08]"
const projectInlineRowActionButtonClass = cn(inlineRowActionButtonClass, "size-[22px] rounded-[6px]")
const projectInlineCollapseButtonClass = cn(
	projectInlineRowActionButtonClass,
	"ml-1 opacity-0 transition-[background-color,color,opacity] group-hover/sidebar-row:opacity-100 group-focus-within/sidebar-row:opacity-100 focus-visible:opacity-100",
)
const projectInlineRowActionIconClass = "size-3.5 stroke-[1.5]"

function sessionActionIcon(actionId: SessionRowActionId) {
	if (actionId === "rename") return <PencilIcon className={rowMenuIconClass} />
	if (actionId === "fork") return <SplitIcon className={rowMenuIconClass} />
	return <TrashIcon className={rowMenuIconClass} />
}

function projectActionIcon(actionId: ProjectRowActionId) {
	if (actionId === "pin") return <PinIcon className={rowMenuIconClass} />
	if (actionId === "reveal") return <FolderOpenIcon className={rowMenuIconClass} />
	if (actionId === "create-worktree") return <ArrowUpRightIcon className={rowMenuIconClass} />
	if (actionId === "rename") return <PencilIcon className={rowMenuIconClass} />
	if (actionId === "archive-chats") return <ArchiveIcon className={rowMenuIconClass} />
	return <TrashIcon className={rowMenuIconClass} />
}

function ActionMenuItems<TId extends string>({
	actions,
	iconForAction,
	onAction,
}: {
	actions: SidebarRowAction<TId>[]
	iconForAction: (actionId: TId) => ReactNode
	onAction: (actionId: TId) => void
}) {
	return actions.map((action, index) => (
		<Fragment key={action.id}>
			{index > 0 && action.variant === "destructive" && (
				<DropdownMenuSeparator className={optionMenuSeparatorClass} />
			)}
			<DropdownMenuItem
				disabled={action.disabled}
				variant={action.variant}
				className={rowMenuItemClass}
				onClick={(event) => {
					event.stopPropagation()
					if (action.disabled) return
					onAction(action.id)
				}}
			>
				{iconForAction(action.id)}
				<span className="min-w-0 flex-1 truncate">{action.label}</span>
			</DropdownMenuItem>
		</Fragment>
	))
}

function RowActionsDropdown<TId extends string>({
	actions,
	label,
	iconForAction,
	onAction,
	triggerClassName = sessionRowActionButtonClass,
	triggerIconClassName = "size-4",
	contentSide = "right",
	contentAlign = "start",
	contentClassName = projectMenuContentClass,
}: {
	actions: SidebarRowAction<TId>[]
	label: string
	iconForAction: (actionId: TId) => ReactNode
	onAction: (actionId: TId) => void
	triggerClassName?: string
	triggerIconClassName?: string
	contentSide?: "top" | "right" | "bottom" | "left"
	contentAlign?: "start" | "center" | "end"
	contentClassName?: string
}) {
	if (actions.length === 0) return null

	return (
		<DropdownMenu>
			<DropdownMenuTrigger
				render={
					<button
						type="button"
						aria-label={label}
						className={triggerClassName}
						onClick={(event) => event.stopPropagation()}
						onMouseDown={(event) => event.stopPropagation()}
					>
						<MoreHorizontalIcon className={triggerIconClassName} />
					</button>
				}
			/>
			<DropdownMenuContent
				align={contentAlign}
				alignOffset={-4}
				side={contentSide}
				sideOffset={6}
				className={contentClassName}
			>
				<DropdownMenuGroup>
					<ActionMenuItems actions={actions} iconForAction={iconForAction} onAction={onAction} />
				</DropdownMenuGroup>
			</DropdownMenuContent>
		</DropdownMenu>
	)
}

function SessionContextMenuItems({
	actions,
	onAction,
}: {
	actions: SidebarRowAction<SessionRowActionId>[]
	onAction: (actionId: SessionRowActionId) => void
}) {
	return actions.map((action, index) => (
		<Fragment key={action.id}>
			{index > 0 && action.variant === "destructive" && (
				<ContextMenuSeparator className={optionMenuSeparatorClass} />
			)}
			<ContextMenuItem
				variant={action.variant}
				className={rowMenuItemClass}
				onClick={(event) => {
					event.stopPropagation()
					onAction(action.id)
				}}
			>
				{sessionActionIcon(action.id)}
				<span className="min-w-0 flex-1 truncate">{action.label}</span>
			</ContextMenuItem>
		</Fragment>
	))
}

export const ProjectRow = memo(function ProjectRow({
	project,
	isSelected,
	showCount,
	isCollapsed,
	canToggleSessions,
	onSelect,
	onToggleCollapsed,
	onNewChat,
	onRevealInFinder,
	onRemoveProject,
	isUnavailable,
}: {
	project: SidebarProject
	isSelected: boolean
	showCount: boolean
	isCollapsed: boolean
	canToggleSessions: boolean
	onSelect: () => void
	onToggleCollapsed?: () => void
	onNewChat?: () => void
	onRevealInFinder?: () => void
	onRemoveProject?: () => void
	isUnavailable?: boolean
}) {
	const actions = buildProjectRowActions({ canRevealInFinder: !!onRevealInFinder })
	const handleAction = useCallback(
		(actionId: ProjectRowActionId) => {
			if (actionId === "reveal") {
				onRevealInFinder?.()
				return
			}
			if (actionId === "remove") {
				onRemoveProject?.()
				return
			}
			onSelect()
		},
		[onRemoveProject, onRevealInFinder, onSelect],
	)
	const handleNewChat = useCallback(
		(event: MouseEvent<HTMLButtonElement>) => {
			event.stopPropagation()
			const startNewChat = onNewChat ?? onSelect
			startNewChat()
		},
		[onNewChat, onSelect],
	)
	const handleToggle = useCallback(
		(event: MouseEvent<HTMLButtonElement>) => {
			event.stopPropagation()
			onToggleCollapsed?.()
		},
		[onToggleCollapsed],
	)
	const CollapseIcon = isCollapsed ? ChevronRightIcon : ChevronDownIcon
	const ProjectFolderIcon = canToggleSessions && !isCollapsed ? FolderOpenIcon : FolderClosedIcon

	return (
		<div
			aria-disabled={isUnavailable || undefined}
			data-folder-unavailable={isUnavailable || undefined}
			onClick={onSelect}
			className={cn(
				"group/sidebar-row relative flex h-8 w-full items-center rounded-lg text-sidebar-foreground transition-colors hover:bg-black/[0.04] dark:hover:bg-white/[0.06]",
				isSelected && "bg-black/[0.07] dark:bg-white/[0.08]",
				isUnavailable && "text-muted-foreground opacity-55",
			)}
		>
			<div className="flex h-full min-w-0 flex-1 items-center pr-[52px]">
				<button
					type="button"
					onClick={(event) => {
						event.stopPropagation()
						onSelect()
					}}
					className="flex h-full min-w-0 items-center gap-2.5 rounded-lg py-0 pr-1 pl-1.5 text-left text-[13px] leading-none"
				>
					<ProjectFolderIcon
						className={cn(
							sidebarPrimaryIconClass,
							"shrink-0 text-sidebar-foreground/90",
							isUnavailable && "text-muted-foreground",
						)}
					/>
					<span className="min-w-0 truncate font-normal tracking-normal">{project.name}</span>
					{showCount && project.agentCount > 0 && (
						<span className="flex size-6 shrink-0 items-center justify-center rounded-full bg-muted text-xs font-medium text-muted-foreground transition-opacity duration-150 group-hover/sidebar-row:opacity-0 group-focus-within/sidebar-row:opacity-0">
							{project.agentCount}
						</span>
					)}
				</button>
				{canToggleSessions && (
					<button
						type="button"
						aria-label={isCollapsed ? `Expand ${project.name}` : `Collapse ${project.name}`}
						aria-pressed={isCollapsed}
						className={projectInlineCollapseButtonClass}
						onClick={handleToggle}
						onMouseDown={(event) => event.stopPropagation()}
					>
						<CollapseIcon className={projectInlineRowActionIconClass} />
					</button>
				)}
			</div>
			<div className="absolute right-1 top-1/2 flex -translate-y-1/2 items-center gap-0.5 opacity-0 transition-opacity duration-150 group-hover/sidebar-row:opacity-100 group-focus-within/sidebar-row:opacity-100">
				<RowActionsDropdown
					actions={actions}
					label={`Project actions for ${project.name}`}
					iconForAction={projectActionIcon}
					onAction={handleAction}
					triggerClassName={projectInlineRowActionButtonClass}
					triggerIconClassName={projectInlineRowActionIconClass}
					contentSide="bottom"
					contentAlign="end"
					contentClassName={projectMenuContentClass}
				/>
				<button
					type="button"
					aria-label={`New session in ${project.name}`}
					className={projectInlineRowActionButtonClass}
					onClick={handleNewChat}
					onMouseDown={(event) => event.stopPropagation()}
				>
					<PenLineIcon className={projectInlineRowActionIconClass} />
				</button>
			</div>
		</div>
	)
})

export const SessionRow = memo(function SessionRow({
	agent,
	isSelected,
	onRename,
	onDelete,
	onFork,
	showProject,
	projectUnavailable,
	onUnavailableProject,
}: {
	agent: Agent
	isSelected: boolean
	onRename?: (agent: Agent, title: string) => Promise<void>
	onDelete?: (agent: Agent) => Promise<void>
	onFork?: (agent: Agent) => Promise<void>
	showProject?: boolean
	projectUnavailable?: boolean
	onUnavailableProject?: () => void
}) {
	const navigate = useNavigate()
	const [, startTransition] = useTransition()
	const lastActive = useLiveLastActive(agent.lastActiveAt)
	const isWorktree = !!agent.worktreePath
	const hasWideStatusIndicator = false
	const [isEditing, setIsEditing] = useState(false)
	const [editValue, setEditValue] = useState(agent.name)
	const inputRef = useRef<HTMLInputElement>(null)

	const onSelect = useCallback(() => {
		if (projectUnavailable) {
			onUnavailableProject?.()
			return
		}
		startTransition(() => {
			appStore.set(customizeOpenAtom, false)
			navigate({
				to: "/project/$projectSlug/session/$sessionId",
				params: { projectSlug: agent.projectSlug, sessionId: agent.id },
			})
		})
	}, [agent.id, agent.projectSlug, navigate, onUnavailableProject, projectUnavailable])

	const startEditing = useCallback(() => {
		setEditValue(agent.name)
		setIsEditing(true)
	}, [agent.name])

	const confirmRename = useCallback(async () => {
		const trimmed = editValue.trim()
		setIsEditing(false)
		if (trimmed && trimmed !== agent.name && onRename) {
			await onRename(agent, trimmed)
		}
	}, [editValue, agent, onRename])

	const cancelEditing = useCallback(() => {
		setIsEditing(false)
		setEditValue(agent.name)
	}, [agent.name])

	const sessionActions = buildSessionRowActions({
		canRename: !!onRename,
		canFork: !!onFork,
		canDelete: !!onDelete,
	})
	const hasSessionActions = sessionActions.length > 0
	const handleSessionAction = useCallback(
		(actionId: SessionRowActionId) => {
			if (actionId === "rename") {
				startEditing()
				return
			}
			if (actionId === "fork") {
				onFork?.(agent)
				return
			}
			onDelete?.(agent)
		},
		[agent, onDelete, onFork, startEditing],
	)

	useEffect(() => {
		if (isEditing && inputRef.current) {
			inputRef.current.focus()
			inputRef.current.select()
		}
	}, [isEditing])

	const row = (
		<div
			aria-disabled={projectUnavailable || undefined}
			data-folder-unavailable={projectUnavailable || undefined}
			className={cn(
				"group/sidebar-row relative flex min-h-8 w-full items-center rounded-lg text-sidebar-foreground transition-colors",
				!isSelected && "hover:bg-black/[0.04] dark:hover:bg-white/[0.06]",
				isSelected && "bg-black/[0.07] dark:bg-white/[0.08]",
				projectUnavailable && "text-muted-foreground opacity-55",
			)}
		>
			{!isEditing && agent.status === "running" && (
				<span
					className="pointer-events-none absolute left-1.5 top-1/2 -translate-y-1/2"
					aria-hidden="true"
				>
					<Loader2Icon className="size-3.5 animate-spin text-[#3396f4]" />
				</span>
			)}
			<button
				type="button"
				onClick={isEditing ? undefined : onSelect}
				className={cn(
					"flex min-h-8 min-w-0 flex-1 items-center gap-2 rounded-lg py-1 pl-[34px] text-left",
					sessionTitleTextClass,
					hasWideStatusIndicator ? "pr-28" : "pr-12",
				)}
			>
				{isEditing ? (
					<input
						ref={inputRef}
						value={editValue}
						aria-label="Session name"
						onChange={(event) => setEditValue(event.target.value)}
						onKeyDown={(event) => {
							event.stopPropagation()
							if (event.key === "Enter") confirmRename()
							if (event.key === "Escape") cancelEditing()
						}}
						onBlur={confirmRename}
						onClick={(event) => event.stopPropagation()}
						className="min-w-0 flex-1 appearance-none border-0 bg-transparent p-0 [font:inherit] text-inherit outline-none"
					/>
				) : (
					<div className="min-w-0 flex-1">
						<span className="block truncate">{agent.name}</span>
						{showProject && (
							<span className="block truncate text-[11px] leading-4 text-muted-foreground">
								{agent.project}
							</span>
						)}
					</div>
				)}
			</button>
			{!isEditing && (
				<div
					className={cn(sessionStatusContainerClass, hasWideStatusIndicator && "min-w-[86px]")}
					onClick={onSelect}
				>
					<span
						className={cn(
							"pointer-events-none flex h-7 items-center",
							hasSessionActions && sessionStatusHiddenClass,
						)}
					>
						<SessionRowStatusIndicator agent={agent} lastActive={lastActive} isWorktree={isWorktree} />
					</span>
					{hasSessionActions && (
						<RowActionsDropdown
							actions={sessionActions}
							label={`Session actions for ${agent.name}`}
							iconForAction={sessionActionIcon}
							onAction={handleSessionAction}
							contentClassName={sessionMenuContentClass}
						/>
					)}
				</div>
			)}
		</div>
	)

	return (
		<ContextMenu>
			<ContextMenuTrigger render={row} />
			<ContextMenuContent className={sessionMenuContentClass}>
				<SessionContextMenuItems actions={sessionActions} onAction={handleSessionAction} />
			</ContextMenuContent>
		</ContextMenu>
	)
})
