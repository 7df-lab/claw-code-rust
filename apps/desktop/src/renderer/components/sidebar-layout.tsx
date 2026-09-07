/**
 * Sidebar shell layout: wraps child routes with the sidebar + SidebarInset chrome.
 * Reads from SidebarSlotContext to allow child routes to override sidebar content.
 */
import { Button } from "@devo/ui/components/button"
import {
	Sidebar,
	SidebarHeader,
	SidebarInset,
	SidebarProvider,
	useSidebar,
} from "@devo/ui/components/sidebar"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { cn } from "@devo/ui/lib/utils"
import { Outlet, useNavigate, useParams, useRouterState } from "@tanstack/react-router"
import { useAtom, useAtomValue, useSetAtom } from "jotai"
import { type MouseEvent, useCallback, useEffect, useRef, useState } from "react"
import { toast } from "sonner"
import type { DesktopFolder, DesktopFolderStatus } from "../../preload/api"
import {
	desktopFolderStatusByDirectoryAtom,
	desktopFoldersAtom,
} from "../atoms/desktop-folders"
import { openNewTerminalAtom, terminalPanelOpenAtom } from "../atoms/terminal"
import { customizeOpenAtom, settingsBackgroundSessionAtom, settingsOverlayOpenAtom } from "../atoms/ui"
import { lastProjectDirectoryAtom, sidebarWidthAtom } from "../atoms/preferences"
import { clampSidebarWidth } from "../lib/sidebar-width"
import { useAppRoutePersistence } from "../hooks/use-app-route-persistence"
import { useAgents, useProjectList, useSetCommandPaletteOpen } from "../hooks/use-agents"
import { useAgentActions } from "../hooks/use-server"
import {
	buildDesktopFolder,
	desktopFolderProjectSlug,
	normalizeDesktopFolderDirectory,
	removeDesktopFolder,
	upsertDesktopFolder,
} from "../lib/desktop-folders"
import { formatShortcut } from "../lib/shortcut-display"
import { navigateToNewChat } from "../lib/project-selection"
import { isSettingsRoute } from "../lib/app-navigation"
import type { Agent, SidebarProject } from "../lib/types"
import { createDesktopFolder, pickDirectory, statDesktopFolders } from "../services/backend"
import {
	deleteProjectSessions,
	loadProjectSessions,
	refillProjectSessionsAfterDelete,
} from "../services/connection-manager"
import { APP_BAR_HEIGHT, AppBar } from "./app-bar"
import { CustomizeView } from "./customize/customize-view"
import { DesktopProjectActionsProvider } from "./desktop-project-actions-context"
import { DesktopTerminalPanel } from "./desktop-terminal-panel"
import { LeftPanelIcon } from "./panel-icons"
import { AppSidebarContent } from "./sidebar"
import { SidebarResizeHandle } from "./sidebar/sidebar-resize-handle"
import {
	SessionDeleteDialog,
	deleteSessionNavigationTarget,
} from "./sidebar/sidebar-session-delete"
import { CreateFolderDialog } from "./sidebar/sidebar-folder-dialogs"
import { useSidebarSlot } from "./sidebar-slot-context"
import { SessionShell } from "./session-shell"
import { UpdateBanner } from "./update-banner"

// ============================================================
// Constants
// ============================================================

const isMac =
	typeof window !== "undefined" && "devo" in window && window.devo.platform === "darwin"
const isElectronEnv = typeof window !== "undefined" && "devo" in window
const isWindowsElectron = isElectronEnv && window.devo.platform === "win32"

/** Pixel offset from the left edge where window controls start */
const WINDOW_CONTROLS_LEFT = isMac && isElectronEnv ? 93 : 8
const WINDOW_CONTROLS_TOP = isMac && isElectronEnv ? 7 : 6
/** Total width reserved for traffic lights or custom titlebar controls */
const WINDOW_CONTROLS_INSET = isMac && isElectronEnv ? 160 : isWindowsElectron ? 248 : 72
const WINDOW_CONTROLS_RIGHT_INSET = isWindowsElectron ? 138 : 12

// ============================================================
// NarrowWindowCollapser
// ============================================================

/**
 * Watches the window width and auto-collapses the sidebar when it drops below
 * COLLAPSE_THRESHOLD px, restoring it when the window grows back above the threshold.
 * Must be rendered inside a <SidebarProvider>.
 */
const COLLAPSE_THRESHOLD = 600

function NarrowWindowCollapser() {
	const { open, setOpen } = useSidebar()
	// Track whether the last collapse was triggered by us (vs. the user manually toggling)
	const collapsedByUsRef = useRef(false)

	useEffect(() => {
		const check = () => {
			const narrow = window.innerWidth < COLLAPSE_THRESHOLD
			if (narrow && open) {
				collapsedByUsRef.current = true
				setOpen(false)
			} else if (!narrow && !open && collapsedByUsRef.current) {
				// Only re-open if WE collapsed it — don't override the user's manual close
				collapsedByUsRef.current = false
				setOpen(true)
			} else if (!narrow) {
				// Window grew back — reset the flag regardless so we don't re-open unexpectedly
				if (!open) collapsedByUsRef.current = false
			}
		}

		check()
		window.addEventListener("resize", check)
		return () => window.removeEventListener("resize", check)
	}, [open, setOpen])

	return null
}

// ============================================================
// WindowControls
// ============================================================

const APP_MENU_ITEMS = [
	{ id: "file", label: "File" },
	{ id: "edit", label: "Edit" },
	{ id: "view", label: "View" },
	{ id: "window", label: "Window" },
] as const

function AppMenuBar() {
	const handleMenuClick = useCallback(
		(event: MouseEvent<HTMLButtonElement>, id: (typeof APP_MENU_ITEMS)[number]["id"]) => {
			const rect = event.currentTarget.getBoundingClientRect()
			void window.devo.appMenu.popup(id, {
				x: Math.round(rect.left),
				y: Math.round(rect.bottom),
			})
		},
		[],
	)

	if (!isWindowsElectron) {
		return null
	}

	return (
		<nav aria-label="Application menu" className="ml-2 flex items-center gap-0.5">
			{APP_MENU_ITEMS.map((item) => (
				<button
					key={item.id}
					type="button"
					className="h-7 rounded-md px-2 text-[13px] font-normal text-muted-foreground transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring/50"
					onClick={(event) => handleMenuClick(event, item.id)}
					style={{
						// @ts-expect-error -- vendor-prefixed CSS property
						WebkitAppRegion: "no-drag",
					}}
				>
					{item.label}
				</button>
			))}
		</nav>
	)
}

/**
 * Absolutely positioned window controls that
 * stay next to the macOS traffic lights regardless of sidebar state.
 * Must be rendered inside a SidebarProvider.
 */
function WindowControls() {
	const { isMobile, open, openMobile, toggleSidebar } = useSidebar()
	const toggleSidebarShortcut = formatShortcut(["mod", "B"])
	const sidebarOpen = isMobile ? openMobile : open

	return (
		<div
			className="absolute z-50 flex items-center gap-0.5"
			style={{
				top: WINDOW_CONTROLS_TOP,
				left: WINDOW_CONTROLS_LEFT,
				// @ts-expect-error -- vendor-prefixed CSS property
				WebkitAppRegion: "no-drag",
			}}
		>
			<Tooltip>
				<TooltipTrigger
					render={
						<Button
							variant="ghost"
							size="icon"
							aria-label="Toggle sidebar"
							className="size-7 shrink-0"
							onClick={toggleSidebar}
						/>
					}
				>
					<LeftPanelIcon open={sidebarOpen} className="size-3.5" aria-hidden="true" />
				</TooltipTrigger>
				<TooltipContent>Toggle sidebar ({toggleSidebarShortcut})</TooltipContent>
			</Tooltip>
			<AppMenuBar />
		</div>
	)
}

// ============================================================
// SidebarLayout
// ============================================================

export function SidebarLayout() {
	const navigate = useNavigate()
	const { projectSlug, sessionId } = useParams({ strict: false }) as {
		projectSlug?: string
		sessionId?: string
	}
	const pathname = useRouterState({ select: (state) => state.location.pathname })
	const { content: slotContent, footer: slotFooter } = useSidebarSlot()
	const [terminalPanelOpen, setTerminalPanelOpen] = useAtom(terminalPanelOpenAtom)
	const openNewTerminal = useSetAtom(openNewTerminalAtom)
	const backgroundSession = useAtomValue(settingsBackgroundSessionAtom)
	const [settingsOverlayOpen, setSettingsOverlayOpen] = useAtom(settingsOverlayOpenAtom)
	const [customizeOpen, setCustomizeOpen] = useAtom(customizeOpenAtom)
	useAppRoutePersistence()

	const isSettingsOpen = isSettingsRoute(pathname)
	if (settingsOverlayOpen !== isSettingsOpen) {
		setSettingsOverlayOpen(isSettingsOpen)
	}
	const previousPathnameRef = useRef(pathname)
	if (previousPathnameRef.current !== pathname) {
		previousPathnameRef.current = pathname
		if (customizeOpen) setCustomizeOpen(false)
	}

	// ---- Sidebar-specific data ----
	const agents = useAgents()
	const projects = useProjectList()
	const lastProjectDirectory = useAtomValue(lastProjectDirectoryAtom)
	const setLastProjectDirectory = useSetAtom(lastProjectDirectoryAtom)
	const [sidebarWidth, setSidebarWidth] = useAtom(sidebarWidthAtom)
	const [sidebarResizing, setSidebarResizing] = useState(false)
	const [dragSidebarWidth, setDragSidebarWidth] = useState<number | null>(null)
	const sidebarResizingRef = useRef(false)
	const setCommandPaletteOpen = useSetCommandPaletteOpen()
	const desktopFolders = useAtomValue(desktopFoldersAtom)
	const folderStatuses = useAtomValue(desktopFolderStatusByDirectoryAtom)
	const setDesktopFolders = useSetAtom(desktopFoldersAtom)
	const setFolderStatuses = useSetAtom(desktopFolderStatusByDirectoryAtom)
	const { renameSession, deleteSession, forkSession } = useAgentActions()
	const [deleteTarget, setDeleteTarget] = useState<Agent | null>(null)
	const [deletePending, setDeletePending] = useState(false)
	const [deleteError, setDeleteError] = useState<string | null>(null)
	const [createFolderOpen, setCreateFolderOpen] = useState(false)
	const [createFolderParent, setCreateFolderParent] = useState("")
	const [createFolderName, setCreateFolderName] = useState("")
	const [createFolderPending, setCreateFolderPending] = useState(false)
	const [createFolderError, setCreateFolderError] = useState<string | null>(null)
	const loadedProjectDirectoriesRef = useRef<Set<string>>(new Set())
	const evictKeepAliveSessionRef = useRef<(sessionId: string) => void>(() => {})
	useEffect(() => {
		if (!projectSlug) return
		const project = projects.find((item) => item.slug === projectSlug)
		if (project) setLastProjectDirectory(project.directory)
	}, [projectSlug, projects, setLastProjectDirectory])

	// Sub-agents are filtered at the API level (roots: true)
	const visibleAgents = agents
	const activeAgent = sessionId ? visibleAgents.find((agent) => agent.id === sessionId) : null
	const terminalDirectory = activeAgent?.worktreePath ?? activeAgent?.directory ?? null
	const sessionToKeepAlive =
		sessionId ?? (isSettingsOpen ? backgroundSession?.sessionId : null)
	const isTranscriptRoute =
		pathname.includes("/session/") ||
		(isSettingsOpen && backgroundSession != null) ||
		/^\/automations\/[^/]+\/runs\/[^/]+$/.test(pathname)
	const transcriptFillsTitlebar = isMac && isTranscriptRoute
	const transcriptTitlebarFillAttr = transcriptFillsTitlebar ? "true" : undefined

	const handleRenameSession = useCallback(
		async (agent: Agent, title: string) => {
			await renameSession(agent.directory, agent.sessionId, title)
		},
		[renameSession],
	)

	const handleDeleteSession = useCallback(
		async (agent: Agent) => {
			setDeleteError(null)
			setDeleteTarget(agent)
		},
		[],
	)

	const handleDeleteDialogOpenChange = useCallback((open: boolean) => {
		if (open) return
		setDeleteTarget(null)
		setDeleteError(null)
	}, [])

	const handleConfirmDeleteSession = useCallback(async () => {
		if (!deleteTarget || deletePending) return
		setDeletePending(true)
		setDeleteError(null)
		try {
			await deleteSession(deleteTarget.directory, deleteTarget.sessionId)
			evictKeepAliveSessionRef.current(deleteTarget.sessionId)
			await refillProjectSessionsAfterDelete(
				deleteTarget.projectDirectory,
				deleteTarget.sessionId,
			)
			const navigationTarget = deleteSessionNavigationTarget({
				deletedSessionId: deleteTarget.sessionId,
				currentSessionId: sessionId,
				projectSlug,
			})
			setDeleteTarget(null)
			if (navigationTarget?.to === "/project/$projectSlug") {
				navigate({
					to: "/project/$projectSlug",
					params: navigationTarget.params,
				})
			} else if (navigationTarget?.to === "/") {
				navigate({ to: "/" })
			}
		} catch (err) {
			const message = err instanceof Error ? err.message : "Failed to delete session"
			setDeleteError(message)
			toast.error("Failed to delete session", { description: message })
		} finally {
			setDeletePending(false)
		}
	}, [deletePending, deleteSession, deleteTarget, navigate, projectSlug, sessionId])

	const handleForkSession = useCallback(
		async (agent: Agent) => {
			const forked = await forkSession(agent.directory, agent.sessionId)
			if (forked) {
				navigate({
					to: "/project/$projectSlug/session/$sessionId",
					params: { projectSlug: agent.projectSlug, sessionId: forked.id },
				})
			}
		},
		[forkSession, navigate],
	)

	const handleOpenCommandPalette = useCallback(() => {
		setCommandPaletteOpen(true)
	}, [setCommandPaletteOpen])

	const persistDesktopFolders = useCallback(
		async (folders: DesktopFolder[]) => {
			if (!isElectronEnv) {
				setDesktopFolders(folders)
				return folders
			}
			const settings = await window.devo.updateSettings({ desktopFolders: { folders } })
			setDesktopFolders(settings.desktopFolders.folders)
			return settings.desktopFolders.folders
		},
		[setDesktopFolders],
	)

	useEffect(() => {
		const directories = desktopFolders.map((folder) =>
			normalizeDesktopFolderDirectory(folder.directory),
		)
		if (directories.length === 0) {
			setFolderStatuses({})
			return
		}

		let cancelled = false
		statDesktopFolders(directories)
			.then((results) => {
				if (cancelled) return
				const nextStatuses: Record<string, DesktopFolderStatus> = {}
				for (const result of results) {
					nextStatuses[normalizeDesktopFolderDirectory(result.directory)] = result.status
				}
				setFolderStatuses(nextStatuses)
			})
			.catch((err) => {
				console.error("Failed to stat desktop folders", err)
			})

		return () => {
			cancelled = true
		}
	}, [desktopFolders, setFolderStatuses])

	useEffect(() => {
		for (const folder of desktopFolders) {
			const directory = normalizeDesktopFolderDirectory(folder.directory)
			if (folderStatuses[directory] !== "available") continue
			if (loadedProjectDirectoriesRef.current.has(directory)) continue
			loadedProjectDirectoriesRef.current.add(directory)
			void loadProjectSessions(directory)
		}
	}, [desktopFolders, folderStatuses])

	const handleAddProject = useCallback(async () => {
		try {
			const selectedDirectory = await pickDirectory()
			if (!selectedDirectory) return null
			const normalizedDirectory = normalizeDesktopFolderDirectory(selectedDirectory)
			const [stat] = await statDesktopFolders([normalizedDirectory])
			if (stat?.status !== "available") {
				toast.error("Folder is not available", {
					description:
						stat?.status === "not_directory"
							? "The selected path is not a directory."
							: "The selected folder no longer exists.",
				})
				return null
			}
			const folder = buildDesktopFolder(stat.directory)
			const nextFolders = upsertDesktopFolder(desktopFolders, folder)
			await persistDesktopFolders(nextFolders)
			setFolderStatuses((previous) => ({
				...previous,
				[folder.directory]: "available",
			}))
			loadedProjectDirectoriesRef.current.add(folder.directory)
			await loadProjectSessions(folder.directory)
			navigate({
				to: "/project/$projectSlug",
				params: { projectSlug: desktopFolderProjectSlug(folder) },
			})
			return folder
		} catch (err) {
			const message = err instanceof Error ? err.message : "Failed to add folder"
			toast.error("Failed to add folder", { description: message })
			return null
		}
	}, [desktopFolders, navigate, persistDesktopFolders, setFolderStatuses])

	useEffect(() => {
		if (!isElectronEnv || typeof window.devo.appMenu?.onAction !== "function") return
		return window.devo.appMenu.onAction((action) => {
			if (action === "new-agent") {
				setCustomizeOpen(false)
				navigateToNewChat(navigate, projects, projectSlug, lastProjectDirectory)
				return
			}
			if (action === "open-folder") {
				void handleAddProject()
				return
			}
			if (action === "new-terminal") {
				openNewTerminal()
			}
		})
	}, [
		handleAddProject,
		lastProjectDirectory,
		navigate,
		openNewTerminal,
		projects,
		projectSlug,
		setCustomizeOpen,
	])

	const handleOpenCreateFolder = useCallback(() => {
		setCreateFolderError(null)
		setCreateFolderOpen(true)
	}, [])

	const handlePickCreateFolderParent = useCallback(async () => {
		const directory = await pickDirectory()
		if (directory) setCreateFolderParent(directory)
	}, [])

	const handleCreateFolderDialogOpenChange = useCallback((open: boolean) => {
		if (open) {
			setCreateFolderOpen(true)
			return
		}
		if (createFolderPending) return
		setCreateFolderOpen(false)
		setCreateFolderError(null)
	}, [createFolderPending])

	const handleConfirmCreateFolder = useCallback(async () => {
		if (createFolderPending) return
		const parentDirectory = createFolderParent.trim()
		const name = createFolderName.trim()
		if (!parentDirectory || !name) {
			setCreateFolderError("Choose a parent directory and enter a folder name.")
			return
		}
		setCreateFolderPending(true)
		setCreateFolderError(null)
		try {
			const created = await createDesktopFolder({ parentDirectory, name })
			const folder = buildDesktopFolder(created.directory, created.name)
			const nextFolders = upsertDesktopFolder(desktopFolders, folder)
			await persistDesktopFolders(nextFolders)
			setFolderStatuses((previous) => ({
				...previous,
				[folder.directory]: "available",
			}))
			loadedProjectDirectoriesRef.current.add(folder.directory)
			await loadProjectSessions(folder.directory)
			setCreateFolderOpen(false)
			setCreateFolderParent("")
			setCreateFolderName("")
			navigate({
				to: "/project/$projectSlug",
				params: { projectSlug: desktopFolderProjectSlug(folder) },
			})
		} catch (err) {
			const message = err instanceof Error ? err.message : "Failed to create folder"
			setCreateFolderError(message)
		} finally {
			setCreateFolderPending(false)
		}
	}, [
		createFolderName,
		createFolderParent,
		createFolderPending,
		desktopFolders,
		navigate,
		persistDesktopFolders,
		setFolderStatuses,
	])

	const handleRemoveFolder = useCallback(
		async (project: SidebarProject) => {
			await deleteProjectSessions(project.directory)
			const nextFolders = removeDesktopFolder(desktopFolders, project.directory)
			await persistDesktopFolders(nextFolders)
			setFolderStatuses((previous) => {
				const next = { ...previous }
				delete next[normalizeDesktopFolderDirectory(project.directory)]
				return next
			})
			loadedProjectDirectoriesRef.current.delete(normalizeDesktopFolderDirectory(project.directory))
			if (projectSlug === project.slug) {
				navigate({ to: "/" })
			}
		},
		[desktopFolders, navigate, persistDesktopFolders, projectSlug, setFolderStatuses],
	)

	const handleSidebarWidthChange = useCallback(
		(nextWidth: number) => {
			const clamped = clampSidebarWidth(nextWidth, { windowWidth: window.innerWidth })
			// Live drag uses local state so we don't hit atomWithStorage/localStorage every frame.
			if (sidebarResizingRef.current) {
				setDragSidebarWidth(clamped)
				return
			}
			setSidebarWidth(clamped)
			setDragSidebarWidth(null)
		},
		[setSidebarWidth],
	)

	const handleSidebarResizingChange = useCallback(
		(resizing: boolean) => {
			sidebarResizingRef.current = resizing
			setSidebarResizing(resizing)
			if (!resizing) {
				setDragSidebarWidth((current) => {
					if (current != null) setSidebarWidth(current)
					return null
				})
			}
		},
		[setSidebarWidth],
	)

	const resolvedSidebarWidth = clampSidebarWidth(dragSidebarWidth ?? sidebarWidth, {
		windowWidth: typeof window !== "undefined" ? window.innerWidth : undefined,
	})

	return (
		<div
			className="relative flex h-screen text-foreground"
			style={
				{
					"--window-controls-inset": `${WINDOW_CONTROLS_INSET}px`,
					"--window-controls-right-inset": `${WINDOW_CONTROLS_RIGHT_INSET}px`,
				} as React.CSSProperties
			}
		>
			<DesktopProjectActionsProvider
				startFromScratch={handleOpenCreateFolder}
				useExistingFolder={handleAddProject}
			>
				<SidebarProvider
					embedded
					defaultOpen={true}
					data-resizing={sidebarResizing ? "true" : undefined}
					className="relative"
					style={
						{
							"--sidebar-width": `${resolvedSidebarWidth}px`,
						} as React.CSSProperties
					}
				>
					<NarrowWindowCollapser />
					<Sidebar collapsible="offcanvas" variant="sidebar">
						{/* Sidebar header -- reserves space to match the app bar height so
						 * sidebar content aligns with the main content area. Also clears
						 * the traffic lights + the absolutely-positioned toggle button. */}
						<SidebarHeader
							className="flex-row items-center gap-1 shrink-0 transition-colors duration-150"
							style={{
								height: APP_BAR_HEIGHT,
								// Make header draggable on Electron (acts as title bar above sidebar)
								// @ts-expect-error -- vendor-prefixed CSS property
								WebkitAppRegion: "drag",
							}}
						/>
						{slotContent ?? (
							<>
								<AppSidebarContent
									agents={visibleAgents}
									projects={projects}
									onOpenCommandPalette={handleOpenCommandPalette}
									onCreateFolder={handleOpenCreateFolder}
									onAddProject={handleAddProject}
									onRemoveProject={handleRemoveFolder}
									onRenameSession={handleRenameSession}
									onDeleteSession={handleDeleteSession}
									onForkSession={handleForkSession}
								/>
								<CreateFolderDialog
									open={createFolderOpen}
									parentDirectory={createFolderParent}
									name={createFolderName}
									pending={createFolderPending}
									error={createFolderError}
									onOpenChange={handleCreateFolderDialogOpenChange}
									onPickParent={handlePickCreateFolderParent}
									onParentDirectoryChange={setCreateFolderParent}
									onNameChange={setCreateFolderName}
									onSubmit={handleConfirmCreateFolder}
								/>
								<SessionDeleteDialog
									open={!!deleteTarget}
									pending={deletePending}
									error={deleteError}
									onOpenChange={handleDeleteDialogOpenChange}
									onConfirm={handleConfirmDeleteSession}
								/>
							</>
						)}
						{/* Footer: false = hide, ReactNode = render it, null = let default handle it.
						 * When default sidebar is active, AppSidebarContent renders its own footer. */}
						{slotFooter !== false && slotFooter}
					</Sidebar>
					<SidebarResizeHandle
						width={resolvedSidebarWidth}
						onWidthChange={handleSidebarWidthChange}
						onResizingChange={handleSidebarResizingChange}
					/>
					<SidebarInset data-transcript-titlebar-fill={transcriptTitlebarFillAttr}>
						<UpdateBanner />
						{!transcriptFillsTitlebar && <AppBar />}
						{/* Flex-1 + min-h-0 wrapper: pages use h-full which would
						    resolve to 100% of SidebarInset. This container takes
						    remaining space after the optional AppBar and constrains
						    page content correctly. */}
						<div
							data-slot="content-area"
							data-transcript-titlebar-fill={transcriptTitlebarFillAttr}
							className="relative min-h-0 min-w-0 flex-1 overflow-hidden"
						>
							<div
								className={cn(
									"absolute inset-0 h-full",
									(isSettingsOpen || customizeOpen) && "pointer-events-none invisible",
								)}
								aria-hidden={isSettingsOpen || customizeOpen}
							>
								{sessionToKeepAlive ? (
									<SessionShell
										activeSessionId={sessionToKeepAlive}
										evictRef={evictKeepAliveSessionRef}
									/>
								) : (
									!isSettingsOpen && !customizeOpen && <Outlet />
								)}
							</div>
							{customizeOpen && !isSettingsOpen && (
								<div className="absolute inset-0 z-10 h-full overflow-hidden bg-background">
									<CustomizeView />
								</div>
							)}
							{isSettingsOpen && (
								<div className="absolute inset-0 z-10 h-full overflow-hidden bg-background">
									<Outlet />
								</div>
							)}
						</div>
						<DesktopTerminalPanel
							open={terminalPanelOpen}
							directory={terminalDirectory}
							onOpenChange={setTerminalPanelOpen}
						/>
					</SidebarInset>
					{/* Rendered last so it paints on top of the sidebar and app bar,
					    whose transition properties create stacking contexts. */}
					<WindowControls />
				</SidebarProvider>
			</DesktopProjectActionsProvider>
		</div>
	)
}
