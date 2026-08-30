import { SidebarContent, SidebarFooter } from "@devo/ui/components/sidebar"
import { cn } from "@devo/ui/lib/utils"
import { useNavigate, useParams, useRouterState } from "@tanstack/react-router"
import { useAtom, useAtomValue } from "jotai"
import {
	Clock3Icon,
	FolderPlusIcon,
	Loader2Icon,
	MousePointer2Icon,
	SearchIcon,
	SettingsIcon,
	BlocksIcon,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { activeServerConfigAtom } from "../../atoms/connection"
import { sandboxMappingsAtom } from "../../atoms/derived/agents"
import { automationsEnabledAtom } from "../../atoms/feature-flags"
import { lastProjectDirectoryAtom } from "../../atoms/preferences"
import { projectPaginationFamily } from "../../atoms/sessions"
import { appStore } from "../../atoms/store"
import { customizeOpenAtom, sessionScrollTopFamily } from "../../atoms/ui"
import type { Agent, SidebarProject } from "../../lib/types"
import { freezeSessionScroll } from "../../lib/settings-scroll-freeze"
import { navigateToNewChat } from "../../lib/project-selection"
import { openInTarget } from "../../services/backend"
import { loadMoreProjectSessions, loadProjectSessions } from "../../services/connection-manager"
import {
	buildSidebarItems,
	groupAgentsByProject,
	type SidebarDisplayItem,
} from "./sidebar-data"
import {
	FolderRemoveDialog,
	MissingFolderDialog,
} from "./sidebar-folder-dialogs"
import { AddProjectMenu, SidebarMainMenu } from "./sidebar-menus"
import { sidebarPreferencesAtom } from "./sidebar-preferences"
import { ProjectRow, SessionRow } from "./sidebar-rows"
import { TopActionRow, sidebarPrimaryIconClass } from "./sidebar-top-action"

interface AppSidebarContentProps {
	agents: Agent[]
	projects: SidebarProject[]
	onOpenCommandPalette: () => void
	onCreateFolder?: () => void
	onAddProject?: () => void
	onRemoveProject?: (project: SidebarProject) => Promise<void>
	onRenameSession?: (agent: Agent, title: string) => Promise<void>
	onDeleteSession?: (agent: Agent) => Promise<void>
	onForkSession?: (agent: Agent) => Promise<void>
}

function ProjectSection({
	item,
	selectedProjectSlug,
	selectedSessionId,
	isCollapsed,
	onToggleCollapsed,
	onRevealInFinder,
	onRemoveProject,
	onMissingFolder,
	sandboxDirs,
	onRenameSession,
	onDeleteSession,
	onForkSession,
}: {
	item: Extract<SidebarDisplayItem, { type: "project" }>
	selectedProjectSlug: string | undefined
	selectedSessionId: string | null
	isCollapsed: boolean
	onToggleCollapsed: (directory: string) => void
	onRevealInFinder?: (directory: string) => void
	onRemoveProject: (project: SidebarProject) => void
	onMissingFolder: (project: SidebarProject) => void
	sandboxDirs: Set<string> | undefined
	onRenameSession?: (agent: Agent, title: string) => Promise<void>
	onDeleteSession?: (agent: Agent) => Promise<void>
	onForkSession?: (agent: Agent) => Promise<void>
}) {
	const navigate = useNavigate()
	const pagination = useAtomValue(projectPaginationFamily(item.project.directory))
	const canShowSessions = true
	const isUnavailable = item.project.folderStatus ? item.project.folderStatus !== "available" : false

	useEffect(() => {
		if (isCollapsed || isUnavailable) return
		if (pagination.loaded || pagination.loading) return
		loadProjectSessions(item.project.directory, sandboxDirs, { limit: 5, roots: true })
	}, [
		isCollapsed,
		isUnavailable,
		item.project.directory,
		pagination.loaded,
		pagination.loading,
		sandboxDirs,
	])

	const handleProjectSelect = useCallback(() => {
		if (isUnavailable) {
			onMissingFolder(item.project)
			return
		}
		if (canShowSessions && !isCollapsed && !pagination.loaded && !pagination.loading) {
			loadProjectSessions(item.project.directory, sandboxDirs, { limit: 5, roots: true })
		}
		navigate({
			to: "/project/$projectSlug",
			params: { projectSlug: item.project.slug },
		})
	}, [
		item.project.directory,
		item.project.slug,
		navigate,
		onMissingFolder,
		pagination.loaded,
		pagination.loading,
		sandboxDirs,
		canShowSessions,
		isCollapsed,
		isUnavailable,
	])

	const handleNewChat = useCallback(() => {
		if (isUnavailable) {
			onMissingFolder(item.project)
			return
		}
		appStore.set(customizeOpenAtom, false)
		navigate({
			to: "/project/$projectSlug",
			params: { projectSlug: item.project.slug },
		})
	}, [isUnavailable, item.project, navigate, onMissingFolder])

	const handleToggleCollapsed = useCallback(() => {
		if (isUnavailable) {
			onMissingFolder(item.project)
			return
		}
		if (canShowSessions && isCollapsed && !pagination.loaded && !pagination.loading) {
			loadProjectSessions(item.project.directory, sandboxDirs, { limit: 5, roots: true })
		}
		onToggleCollapsed(item.project.directory)
	}, [
		canShowSessions,
		isCollapsed,
		isUnavailable,
		item.project,
		item.project.directory,
		onMissingFolder,
		onToggleCollapsed,
		pagination.loaded,
		pagination.loading,
		sandboxDirs,
	])

	const handleLoadMore = useCallback(() => {
		loadMoreProjectSessions(item.project.directory, pagination.currentLimit)
	}, [item.project.directory, pagination.currentLimit])

	const handleRevealInFinder = useCallback(() => {
		onRevealInFinder?.(item.project.directory)
	}, [item.project.directory, onRevealInFinder])

	return (
		<section className="flex flex-col">
			<ProjectRow
				project={item.project}
				isSelected={selectedProjectSlug === item.project.slug && !selectedSessionId}
				showCount={false}
				isCollapsed={isCollapsed}
				canToggleSessions={canShowSessions}
				onSelect={handleProjectSelect}
				onToggleCollapsed={handleToggleCollapsed}
				onNewChat={handleNewChat}
				onRevealInFinder={isUnavailable ? undefined : handleRevealInFinder}
				onRemoveProject={() => onRemoveProject(item.project)}
				isUnavailable={isUnavailable}
			/>
			{canShowSessions && (
				<div
					aria-hidden={isCollapsed}
					className={cn(
						"grid transition-[grid-template-rows,opacity] duration-200 ease-out motion-reduce:transition-none",
						isCollapsed ? "grid-rows-[0fr] opacity-0" : "grid-rows-[1fr] opacity-100",
					)}
					inert={isCollapsed}
				>
					<div className={cn("min-h-0 overflow-hidden", isCollapsed && "pointer-events-none")}>
						<div className="flex flex-col gap-y-1">
							{pagination.loading && item.sessions.length === 0 && (
								<div className="flex h-8 items-center gap-2 py-1 pr-1.5 pl-[34px] text-xs text-muted-foreground">
									<Loader2Icon className="size-3.5 animate-spin" />
									Loading
								</div>
							)}
							{(pagination.loaded
								? item.sessions.slice(0, pagination.currentLimit)
								: item.sessions
							).map((agent) => (
								<SessionRow
									key={agent.id}
									agent={agent}
									isSelected={agent.id === selectedSessionId}
									onRename={onRenameSession}
									onDelete={onDeleteSession}
									onFork={onForkSession}
									projectUnavailable={isUnavailable}
									onUnavailableProject={() => onMissingFolder(item.project)}
								/>
							))}
							{pagination.loaded &&
								(pagination.hasMore || item.sessions.length > pagination.currentLimit) &&
								item.sessions.length > 0 && (
								<button
									type="button"
									onClick={handleLoadMore}
									disabled={pagination.loading}
									className="h-8 rounded-lg py-1 pr-1.5 pl-[34px] text-left text-[13px] font-normal text-muted-foreground transition-colors hover:bg-black/[0.03] hover:text-muted-foreground/90 disabled:opacity-60 dark:hover:bg-white/[0.05]"
								>
									{pagination.loading ? "Loading..." : "More"}
								</button>
							)}
						</div>
					</div>
				</div>
			)}
		</section>
	)
}

export function AppSidebarContent({
	agents,
	projects,
	onOpenCommandPalette,
	onCreateFolder,
	onAddProject,
	onRemoveProject,
	onRenameSession,
	onDeleteSession,
	onForkSession,
}: AppSidebarContentProps) {
	const navigate = useNavigate()
	const routeParams = useParams({ strict: false }) as { projectSlug?: string; sessionId?: string }
	const pathname = useRouterState({ select: (state) => state.location.pathname })
	const selectedSessionId = routeParams.sessionId ?? null
	const [preferences, setPreferences] = useAtom(sidebarPreferencesAtom)
	const [collapsedProjectDirs, setCollapsedProjectDirs] = useState<Set<string>>(() => new Set())
	const [removeTarget, setRemoveTarget] = useState<SidebarProject | null>(null)
	const [missingTarget, setMissingTarget] = useState<SidebarProject | null>(null)
	const [folderActionPending, setFolderActionPending] = useState(false)
	const [folderActionError, setFolderActionError] = useState<string | null>(null)
	const { parentToSandboxes } = useAtomValue(sandboxMappingsAtom)
	const automationsEnabled = useAtomValue(automationsEnabledAtom)
	const [customizeOpen, setCustomizeOpen] = useAtom(customizeOpenAtom)
	const activeServer = useAtomValue(activeServerConfigAtom)
	const isLocalServer = activeServer.type === "local"
	const canRevealInFinder = typeof window !== "undefined" && "devo" in window
	const stableProjectOrderRef = useRef<Map<string, number>>(new Map())

	const visibleAgents = useMemo(() => agents.filter((agent) => !agent.parentId), [agents])
	const stableProjectOrder = useMemo(() => {
		const order = stableProjectOrderRef.current
		for (const project of projects) {
			if (!order.has(project.directory)) {
				order.set(project.directory, order.size)
			}
		}
		return order
	}, [projects])
	const projectSessionsByDirectory = useMemo(
		() => groupAgentsByProject(visibleAgents, projects),
		[visibleAgents, projects],
	)
	const sidebarItems = useMemo(
		() =>
			buildSidebarItems({
				projects,
				agents: visibleAgents,
				projectSessionsByDirectory,
				preferences,
				projectOrder: stableProjectOrder,
			}),
		[
			projects,
			visibleAgents,
			projectSessionsByDirectory,
			preferences,
			stableProjectOrder,
		],
	)

	const hasContent = sidebarItems.length > 0

	const lastProjectDirectory = useAtomValue(lastProjectDirectoryAtom)
	const handleNewChat = useCallback(() => {
		setCustomizeOpen(false)
		navigateToNewChat(navigate, projects, routeParams.projectSlug, lastProjectDirectory)
	}, [lastProjectDirectory, navigate, projects, routeParams.projectSlug, setCustomizeOpen])

	const handleToggleProjectCollapsed = useCallback((directory: string) => {
		setCollapsedProjectDirs((previous) => {
			const next = new Set(previous)
			if (next.has(directory)) {
				next.delete(directory)
			} else {
				next.add(directory)
			}
			return next
		})
	}, [])

	const handleRevealInFinder = useCallback((directory: string) => {
		openInTarget(directory, "finder", false).catch((error) => {
			console.error("Failed to reveal project in Finder", error)
		})
	}, [])

	const requestRemoveProject = useCallback((project: SidebarProject) => {
		setFolderActionError(null)
		setRemoveTarget(project)
	}, [])

	const requestMissingFolderRemove = useCallback((project: SidebarProject) => {
		setFolderActionError(null)
		setMissingTarget(project)
	}, [])

	const handleFolderDialogOpenChange = useCallback((open: boolean) => {
		if (open) return
		setRemoveTarget(null)
		setMissingTarget(null)
		setFolderActionError(null)
	}, [])

	const confirmRemoveProject = useCallback(
		async (project: SidebarProject | null) => {
			if (!project || folderActionPending || !onRemoveProject) return
			setFolderActionPending(true)
			setFolderActionError(null)
			try {
				await onRemoveProject(project)
				setCollapsedProjectDirs((previous) => {
					if (!previous.has(project.directory)) return previous
					const next = new Set(previous)
					next.delete(project.directory)
					return next
				})
				setRemoveTarget(null)
				setMissingTarget(null)
			} catch (err) {
				const message = err instanceof Error ? err.message : "Failed to remove folder"
				setFolderActionError(message)
			} finally {
				setFolderActionPending(false)
			}
		},
		[folderActionPending, onRemoveProject],
	)

	return (
		<>
			<SidebarContent className="gap-0 bg-transparent px-0 pb-3">
				<div className="flex shrink-0 flex-col gap-1 px-3 pb-7">
					<TopActionRow
						icon={<MousePointer2Icon className={sidebarPrimaryIconClass} />}
						onClick={handleNewChat}
					>
						New chat
					</TopActionRow>
					<TopActionRow
						icon={<SearchIcon className={sidebarPrimaryIconClass} />}
						onClick={onOpenCommandPalette}
					>
						Search
					</TopActionRow>
					{automationsEnabled && isLocalServer && (
						<TopActionRow
							icon={<Clock3Icon className={sidebarPrimaryIconClass} />}
							onClick={() => {
								setCustomizeOpen(false)
								navigate({ to: "/automations" })
							}}
							isActive={pathname === "/automations" || pathname.startsWith("/automations/")}
						>
							Automations
						</TopActionRow>
					)}
					<TopActionRow
						icon={<BlocksIcon className={sidebarPrimaryIconClass} />}
						onClick={() => setCustomizeOpen(true)}
						isActive={customizeOpen}
					>
						Customize
					</TopActionRow>
				</div>

				<div className="group/projects-header flex h-9 shrink-0 items-center gap-1 px-4">
					<div className="flex min-w-0 flex-1 items-center gap-1 text-[12px] font-medium tracking-wide text-muted-foreground/55 transition-colors group-hover/projects-header:text-muted-foreground/70 group-focus-within/projects-header:text-muted-foreground/70">
						<span className="truncate">Projects</span>
					</div>
					<div className="flex items-center gap-1 opacity-0 transition-opacity duration-150 group-hover/projects-header:opacity-100 group-focus-within/projects-header:opacity-100">
						<SidebarMainMenu
							preferences={preferences}
							onPreferencesChange={setPreferences}
							onOpenCommandPalette={onOpenCommandPalette}
						/>
						<AddProjectMenu onCreateFolder={onCreateFolder} onAddExistingFolder={onAddProject} />
					</div>
				</div>

				{!hasContent && (
					<div className="flex flex-1 items-center justify-center p-6">
						<div className="flex max-w-[240px] flex-col items-center gap-3 text-center">
							<div className="flex flex-col gap-1">
								<p className="text-sm text-muted-foreground">No projects yet</p>
								<p className="text-xs text-muted-foreground/70">
									Add an existing project or create a new one to start.
								</p>
							</div>
							{onAddProject && (
								<button
									type="button"
									onClick={onAddProject}
									className="flex h-8 items-center gap-2 rounded-lg px-2 text-sm font-normal text-muted-foreground transition-colors hover:bg-black/[0.04] hover:text-sidebar-foreground focus-visible:bg-black/[0.04] focus-visible:text-sidebar-foreground focus-visible:outline-none dark:hover:bg-white/[0.06] dark:focus-visible:bg-white/[0.06]"
								>
									<FolderPlusIcon className={sidebarPrimaryIconClass} />
									<span>Use existing folder</span>
								</button>
							)}
						</div>
					</div>
				)}

				{hasContent && (
					<div className="scrollbar-comfort flex min-h-0 flex-1 flex-col gap-4 overflow-auto px-3 pb-2">
						<div className="flex flex-col gap-1">
							{sidebarItems.map((item) => (
								<ProjectSection
									key={item.project.id}
									item={item}
									selectedProjectSlug={routeParams.projectSlug}
									selectedSessionId={selectedSessionId}
									isCollapsed={collapsedProjectDirs.has(item.project.directory)}
									onToggleCollapsed={handleToggleProjectCollapsed}
									onRemoveProject={requestRemoveProject}
									onMissingFolder={requestMissingFolderRemove}
									onRevealInFinder={canRevealInFinder ? handleRevealInFinder : undefined}
									sandboxDirs={parentToSandboxes.get(item.project.directory)}
									onRenameSession={onRenameSession}
									onDeleteSession={onDeleteSession}
									onForkSession={onForkSession}
								/>
							))}
						</div>
					</div>
				)}
			</SidebarContent>

			<SidebarFooter className="gap-1 px-3 pt-0 pb-3">
				<button
					type="button"
					onClick={() => {
						setCustomizeOpen(false)
						if (selectedSessionId) {
							const scrollTop = appStore.get(sessionScrollTopFamily(selectedSessionId))
							if (scrollTop != null) {
								freezeSessionScroll(selectedSessionId, scrollTop)
							}
						}
						navigate({ to: "/settings" })
					}}
					className={cn(
						"flex h-8 w-full items-center gap-2.5 rounded-lg px-1.5 text-left text-[13px] font-normal text-muted-foreground transition-colors hover:bg-black/[0.04] hover:text-sidebar-foreground dark:hover:bg-white/[0.06]",
					)}
				>
					<SettingsIcon className={sidebarPrimaryIconClass} />
					<span className="truncate">Settings</span>
				</button>
			</SidebarFooter>
			<FolderRemoveDialog
				project={removeTarget}
				open={!!removeTarget}
				pending={folderActionPending}
				error={folderActionError}
				onOpenChange={handleFolderDialogOpenChange}
				onConfirm={() => confirmRemoveProject(removeTarget)}
			/>
			<MissingFolderDialog
				project={missingTarget}
				open={!!missingTarget}
				pending={folderActionPending}
				error={folderActionError}
				onOpenChange={handleFolderDialogOpenChange}
				onConfirmRemove={() => confirmRemoveProject(missingTarget)}
			/>
		</>
	)
}
