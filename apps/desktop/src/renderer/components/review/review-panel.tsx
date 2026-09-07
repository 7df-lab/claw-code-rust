/**
 * ReviewPanel -- right-side collapsible panel for viewing session file diffs.
 *
 * Performance strategy (layered):
 * 1. Generated files (lockfiles, build output) start collapsed -- no render cost
 * 2. When >AUTO_COLLAPSE_THRESHOLD files, ALL diffs start collapsed (header only)
 * 3. Large diffs (>LARGE_DIFF_LINE_THRESHOLD lines) show a "Load diff" gate
 * 4. TanStack Virtual virtualizes the diff list -- only visible items are in the DOM
 * 5. @pierre/diffs WorkerPoolContext offloads Shiki highlighting to web workers
 * 6. Stable memoized objects prevent @pierre/diffs re-parsing unchanged content
 * 7. Only the active theme (light/dark) is rendered, not both
 */
import {
	DropdownMenu,
	DropdownMenuCheckboxItem,
	DropdownMenuContent,
	DropdownMenuGroup,
	DropdownMenuItem,
	DropdownMenuLabel,
	DropdownMenuRadioGroup,
	DropdownMenuRadioItem,
	DropdownMenuSeparator,
	DropdownMenuTrigger,
} from "@devo/ui/components/dropdown-menu"
import { cn } from "@devo/ui/lib/utils"
import { FileDiff, MultiFileDiff, PatchDiff, useWorkerPool, WorkerPoolContextProvider } from "@pierre/diffs/react"
import { useVirtualizer } from "@tanstack/react-virtual"
import { useAtom, useAtomValue, useSetAtom } from "jotai"
import {
	AlertTriangleIcon,
	CheckIcon,
	ChevronDownIcon,
	ChevronRightIcon,
	ChevronsDownUpIcon,
	ChevronsUpDownIcon,
	EllipsisIcon,
	Loader2Icon,
	MaximizeIcon,
	MinimizeIcon,
	MinusIcon,
	PlusIcon,
	RefreshCwIcon,
	SearchIcon,
	XIcon,
} from "lucide-react"
import {
	memo,
	type ReactNode,
	startTransition,
	useCallback,
	useEffect,
	useMemo,
	useRef,
	useState,
} from "react"
import {
	type DiffStyle,
	reviewPanelOpenAtom,
	reviewPanelSelectedFileAtom,
	reviewPanelWorkspaceUiFamily,
	reviewPanelSettingsAtom,
} from "../../atoms/ui"
import { isMockModeAtom } from "../../atoms/mock-mode"
import { appStore } from "../../atoms/store"
import {
	workspaceChangesStateFamily,
} from "../../atoms/workspace-changes"
import { useVcs } from "../../hooks/use-devo-data"
import { useWorkspaceChanges, useWorkspaceScopeStatsPreview } from "../../hooks/use-workspace-changes"
import type { WorkspaceChangeScope, WorkspaceChangeView } from "../../lib/types"
import {
	type WorkspacePatchFile,
	workspaceChangeStats,
	workspacePatchFilesFromView,
} from "../../lib/workspace-diff"
import { buildExpandableGitFileDiff } from "../../lib/review-file-diff"
import { fetchGitBranches, isElectron } from "../../services/backend"
import { CommitPushDialog } from "./commit-push-dialog"

// ============================================================
// Constants
// ============================================================

/**
 * When the total file count exceeds this threshold, all diffs start
 * collapsed (header-only) to avoid mounting dozens of expensive
 * syntax-highlighted views on initial render.
 */
const AUTO_COLLAPSE_THRESHOLD = 12

const EMPTY_TOGGLES: Record<string, boolean> = {}

/** Max total lines changed before a diff shows a "Load diff" gate. */
const LARGE_DIFF_LINE_THRESHOLD = 1500

/** Estimated height (px) of a collapsed diff section (header only). */
const COLLAPSED_ROW_HEIGHT = 32

/** Hide pierre's empty right-side separator stub + reserved scrollbar gutter.
 *
 * For "N unmodified lines" rows, pierre paints a `[data-separator]` cell in both
 * gutters. The additions-side wrapper is `display:none`, but the cell itself still
 * keeps `--diffs-bg-separator`, which shows up as a small gray block on the right.
 */
const REVIEW_DIFF_UNSAFE_CSS = `
:host {
	--diffs-scrollbar-gutter-override: 0px;
}
[data-additions] [data-gutter] [data-separator=line-info] [data-separator-wrapper],
[data-additions] [data-gutter] [data-separator=line-info-basic] [data-separator-wrapper] {
	display: none !important;
}
[data-additions] [data-gutter] [data-separator],
[data-additions] [data-gutter] [data-gutter-buffer] {
	background-color: var(--diffs-bg) !important;
}
`

const WORKSPACE_CHANGE_SCOPES: Array<{ scope: WorkspaceChangeScope; label: string }> = [
	{ scope: "turn", label: "Last Agent Turn" },
	{ scope: "uncommitted", label: "Uncommitted" },
	{ scope: "unstaged", label: "Unstaged" },
	{ scope: "staged", label: "Staged" },
	{ scope: "branch", label: "Branch" },
]

function scopeLabel(scope: WorkspaceChangeScope): string {
	return WORKSPACE_CHANGE_SCOPES.find((item) => item.scope === scope)?.label ?? "Changes"
}

function statsFromView(view: WorkspaceChangeView | null | undefined): {
	fileCount: number
	additions: number
	deletions: number
} {
	return workspaceChangeStats(view)
}

// ============================================================
// Generated / vendor file detection
// ============================================================

/**
 * Patterns for files considered "generated" -- lockfiles, build output, vendored
 * deps, etc. These files are always shown in the panel, but their diff sections
 * start collapsed so they don't slow down initial render.
 */
const GENERATED_FILE_PATTERNS: RegExp[] = [
	/(?:^|\/)bun\.lock$/,
	/(?:^|\/)bun\.lockb$/,
	/(?:^|\/)package-lock\.json$/,
	/(?:^|\/)yarn\.lock$/,
	/(?:^|\/)pnpm-lock\.yaml$/,
	/(?:^|\/)Gemfile\.lock$/,
	/(?:^|\/)Cargo\.lock$/,
	/(?:^|\/)composer\.lock$/,
	/(?:^|\/)poetry\.lock$/,
	/(?:^|\/)Pipfile\.lock$/,
	/(?:^|\/)go\.sum$/,
	/(?:^|\/)flake\.lock$/,
	/(?:^|\/)dist\//,
	/(?:^|\/)build\//,
	/(?:^|\/)\.next\//,
	/(?:^|\/)out\//,
	/(?:^|\/)vendor\//,
	/(?:^|\/)node_modules\//,
	/\.map$/,
	/\.min\.(js|css)$/,
	/(?:^|\/)\.generated\./,
	/\.g\.(ts|js)$/,
	/\.gen\.(ts|js)$/,
]

function isGeneratedFile(filePath: string): boolean {
	return GENERATED_FILE_PATTERNS.some((p) => p.test(filePath))
}

function isLargeDiff(diff: WorkspacePatchFile): boolean {
	return diff.additions + diff.deletions > LARGE_DIFF_LINE_THRESHOLD
}

// ============================================================
// Worker pool factory (Vite-compatible)
// ============================================================

/**
 * Creates a new Web Worker for the @pierre/diffs worker pool.
 * Uses Vite's `?worker` import pattern for correct bundling.
 */
function workerFactory(): Worker {
	return new Worker(new URL("@pierre/diffs/worker/worker.js", import.meta.url), {
		type: "module",
	})
}

/** Stable pool options object (never changes, avoids provider re-renders). */
const WORKER_POOL_OPTIONS = {
	workerFactory,
	poolSize: 4,
} as const

// ============================================================
// Theme detection (render only one theme, not both)
// ============================================================

function useIsDarkMode(): boolean {
	const [dark, setDark] = useState(
		() =>
			document.documentElement.classList.contains("dark") ||
			document.documentElement.dataset.theme === "dark",
	)
	useEffect(() => {
		const observer = new MutationObserver(() => {
			setDark(
				document.documentElement.classList.contains("dark") ||
					document.documentElement.dataset.theme === "dark",
			)
		})
		observer.observe(document.documentElement, {
			attributes: true,
			attributeFilter: ["class", "data-theme"],
		})
		return () => observer.disconnect()
	}, [])
	return dark
}

// ============================================================
// Main ReviewPanel component
// ============================================================

interface ReviewPanelProps {
	sessionId: string
	directory: string
	className?: string
	/** When true, chrome (close/expand) lives on the parent RightPanel host. */
	embedded?: boolean
}

export const ReviewPanel = memo(function ReviewPanel({
	sessionId,
	directory,
	className,
	embedded = false,
}: ReviewPanelProps) {
	const [workspaceUi, setWorkspaceUi] = useAtom(reviewPanelWorkspaceUiFamily(directory || "\0"))
	const scope = workspaceUi.scope
	const baseBranch = workspaceUi.baseBranch
	const filterOpen = workspaceUi.filterOpen
	const filterQuery = workspaceUi.filterQuery
	const userToggles = workspaceUi.userTogglesByScope[scope] ?? EMPTY_TOGGLES
	const [settings, setSettings] = useAtom(reviewPanelSettingsAtom)
	const [panelOpen, setOpen] = useAtom(reviewPanelOpenAtom)
	const ignoreWhitespace = settings.ignoreWhitespace ?? false
	const wordWrap = settings.wordWrap ?? false
	const { view, error, refetch, isInitialLoading, isRefreshing, ensureFullPatches, key } =
		useWorkspaceChanges(sessionId, directory, scope, {
			enabled: panelOpen && Boolean(directory),
			ignoreWhitespace,
			baseBranch,
		})
	const scopeStats = useWorkspaceScopeStatsPreview(sessionId, directory, {
		enabled: panelOpen && Boolean(directory),
		ignoreWhitespace,
		baseBranch,
	})
	const diffs = useMemo(() => workspacePatchFilesFromView(view), [view])
	const stats = useMemo(() => statsFromView(view), [view])

	// --- Toolbar extras: branch display, commit dialog, find-in-changes ---
	const { data: vcs, reload: reloadVcs } = useVcs(directory || null)
	const isMockMode = useAtomValue(isMockModeAtom)
	const canCommit = isElectron && !isMockMode
	const [commitOpen, setCommitOpen] = useState(false)

	const setScope = useCallback(
		(next: WorkspaceChangeScope) => setWorkspaceUi((prev) => ({ ...prev, scope: next })),
		[setWorkspaceUi],
	)
	const setBaseBranch = useCallback(
		(next: string | null) => setWorkspaceUi((prev) => ({ ...prev, baseBranch: next })),
		[setWorkspaceUi],
	)
	const setFilterOpen = useCallback(
		(next: boolean) => setWorkspaceUi((prev) => ({ ...prev, filterOpen: next })),
		[setWorkspaceUi],
	)
	const setFilterQuery = useCallback(
		(next: string) => setWorkspaceUi((prev) => ({ ...prev, filterQuery: next })),
		[setWorkspaceUi],
	)
	const patchScopeToggles = useCallback(
		(updater: (prev: Record<string, boolean>) => Record<string, boolean>) => {
			setWorkspaceUi((prev) => {
				const current = prev.userTogglesByScope[prev.scope] ?? {}
				return {
					...prev,
					userTogglesByScope: {
						...prev.userTogglesByScope,
						[prev.scope]: updater(current),
					},
				}
			})
		},
		[setWorkspaceUi],
	)

	const handleCommitted = useCallback(() => {
		void refetch()
		reloadVcs()
	}, [refetch, reloadVcs])

	// --- External file selection (e.g. "View diff" button in tool cards) ---
	const externalFile = useAtomValue(reviewPanelSelectedFileAtom)
	const clearExternalFile = useSetAtom(reviewPanelSelectedFileAtom)
	useEffect(() => {
		if (!externalFile || diffs.length === 0) return
		// The tool card sends an absolute path; diff entries use relative paths.
		const match = diffs.find(
			(d) =>
				d.file === externalFile ||
				externalFile.endsWith(`/${d.file}`) ||
				d.file.endsWith(`/${externalFile}`),
		)
		if (match) {
			patchScopeToggles((prev) => ({ ...prev, [match.file]: true }))
		}
		clearExternalFile(null)
	}, [externalFile, clearExternalFile, diffs, patchScopeToggles])

	const manyFiles = diffs.length > AUTO_COLLAPSE_THRESHOLD
	// Stay collapsed while any text file still lacks a patch, so we never
	// auto-expand a row into an infinite "Loading diff…" state.
	const collapseByDefault =
		manyFiles || diffs.some((diff) => !diff.binary && diff.patchPending)

	const getIsCollapsed = useCallback(
		(diff: WorkspacePatchFile): boolean => {
			// User override takes priority
			if (diff.file in userToggles) return !userToggles[diff.file]
			// Auto-collapse rules
			if (collapseByDefault) return true
			if (isGeneratedFile(diff.file)) return true
			return false
		},
		[userToggles, collapseByDefault],
	)

	const toggleFile = useCallback(
		(file: string) => {
			const willExpand =
				file in userToggles
					? !userToggles[file]
					: collapseByDefault || isGeneratedFile(file)
			patchScopeToggles((prev) => {
				const wasExpanded =
					file in prev ? prev[file] : !(collapseByDefault || isGeneratedFile(file))
				return { ...prev, [file]: !wasExpanded }
			})
			if (willExpand) {
				void ensureFullPatches(file)
			}
		},
		[collapseByDefault, ensureFullPatches, patchScopeToggles, userToggles],
	)

	const collapseAll = useCallback(() => {
		const next: Record<string, boolean> = {}
		for (const d of diffs) next[d.file] = false
		patchScopeToggles(() => next)
	}, [diffs, patchScopeToggles])

	const expandAll = useCallback(() => {
		const next: Record<string, boolean> = {}
		for (const d of diffs) next[d.file] = true
		patchScopeToggles(() => next)
		void (async () => {
			await ensureFullPatches()
			const latest = appStore.get(workspaceChangesStateFamily(key)).view
			if (!latest) return
			const pending = workspacePatchFilesFromView(latest).filter(
				(row) =>
					!row.binary &&
					(row.patchPending || (row.oldText == null && row.newText == null)),
			)
			const concurrency = 4
			for (let i = 0; i < pending.length; i += concurrency) {
				const batch = pending.slice(i, i + concurrency)
				await Promise.all(batch.map((row) => ensureFullPatches(row.file)))
			}
		})()
	}, [diffs, ensureFullPatches, key, patchScopeToggles])

	const allCollapsed =
		diffs.length > 0 && diffs.every((diff) => getIsCollapsed(diff))

	const toggleCollapseExpandAll = useCallback(() => {
		if (allCollapsed) expandAll()
		else collapseAll()
	}, [allCollapsed, collapseAll, expandAll])

	// --- Handlers ---
	const handleClose = useCallback(() => setOpen(false), [setOpen])
	const handleToggleExpanded = useCallback(
		() => setSettings((prev) => ({ ...prev, expanded: !prev.expanded })),
		[setSettings],
	)
	const handleSetDiffStyle = useCallback(
		(style: DiffStyle) => setSettings((prev) => ({ ...prev, diffStyle: style })),
		[setSettings],
	)
	const handleToggleIgnoreWhitespace = useCallback(
		() =>
			startTransition(() => {
				setSettings((prev) => ({
					...prev,
					ignoreWhitespace: !(prev.ignoreWhitespace ?? false),
				}))
			}),
		[setSettings],
	)
	const handleToggleWordWrap = useCallback(
		() => setSettings((prev) => ({ ...prev, wordWrap: !(prev.wordWrap ?? false) })),
		[setSettings],
	)

	// Apply find-in-changes filter
	const displayedDiffs = useMemo(() => {
		const query = filterQuery.trim().toLowerCase()
		if (!query) return diffs
		return diffs.filter(
			(d) => d.file.toLowerCase().includes(query) || (d.patch ?? "").toLowerCase().includes(query),
		)
	}, [diffs, filterQuery])

	return (
		<div className={cn("flex h-full flex-col overflow-hidden bg-background", className)}>
			{/* Toolbar: scope stats · branch · … · Commit & Push · sidebar toggle */}
			<div
				className={cn(
					"flex shrink-0 items-center gap-1.5 px-2",
					embedded ? "h-9 border-b border-border/40" : "border-b border-border px-3 py-1.5",
				)}
			>
				<div className="flex min-w-0 flex-1 items-center gap-1.5 overflow-hidden">
					<ScopeDropdown
						scope={scope}
						additions={stats.additions}
						deletions={stats.deletions}
						scopeStats={scopeStats}
						onScopeChange={setScope}
					/>
					{vcs?.branch && vcs.state !== "not_git" ? (
						<BranchDropdown
							directory={directory}
							currentBranch={vcs.branch}
							baseBranch={baseBranch}
							scope={scope}
							onSelectBranch={(branch) => {
								setBaseBranch(branch)
								setScope("branch")
							}}
						/>
					) : null}
				</div>
				<div className="flex shrink-0 items-center gap-1">
					<OptionsMenu
						settings={settings}
						onSetDiffStyle={handleSetDiffStyle}
						onToggleIgnoreWhitespace={handleToggleIgnoreWhitespace}
						onToggleWordWrap={handleToggleWordWrap}
						onFind={() => setFilterOpen(true)}
						onToggleCollapseExpandAll={toggleCollapseExpandAll}
						allCollapsed={allCollapsed}
						onRefresh={refetch}
						hasChanges={displayedDiffs.length > 0}
					/>
					{canCommit ? (
						<button
							type="button"
							onClick={() => setCommitOpen(true)}
							className="flex h-7 items-center gap-1 rounded-md bg-foreground px-2.5 text-[11px] font-medium text-background transition-opacity hover:opacity-90"
							title="Commit all changes and push"
						>
							Commit & Push
							<ChevronDownIcon className="size-3 opacity-80" aria-hidden="true" />
						</button>
					) : null}
					{!embedded && (
						<>
							<button
								type="button"
								onClick={handleToggleExpanded}
								className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
								title={settings.expanded ? "Restore panel size" : "Expand to full width"}
							>
								{settings.expanded ? (
									<MinimizeIcon className="size-3.5 stroke-[1.5]" />
								) : (
									<MaximizeIcon className="size-3.5 stroke-[1.5]" />
								)}
							</button>
							<button
								type="button"
								onClick={handleClose}
								className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
							>
								<XIcon className="size-3.5 stroke-[1.5]" />
							</button>
						</>
					)}
				</div>
			</div>

			{/* Find in changes filter row */}
			{filterOpen && (
				<div className="flex shrink-0 items-center gap-1.5 border-b border-border px-3 py-1.5">
					<SearchIcon className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
					<input
						autoFocus
						type="text"
						value={filterQuery}
						onChange={(e) => setFilterQuery(e.target.value)}
						onKeyDown={(e) => {
							if (e.key === "Escape") {
								setFilterOpen(false)
								setFilterQuery("")
							}
						}}
						placeholder="Find in changes (path or content)"
						className="h-5 w-full bg-transparent text-[11px] text-foreground outline-none placeholder:text-muted-foreground/60"
					/>
					{filterQuery && (
						<button
							type="button"
							onClick={() => setFilterQuery("")}
							className="rounded p-0.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
							title="Clear filter"
						>
							<XIcon className="size-3" />
						</button>
					)}
					<button
						type="button"
						onClick={() => {
							setFilterOpen(false)
							setFilterQuery("")
						}}
						className="rounded p-0.5 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
						title="Close find"
					>
						<XIcon className="size-3" />
					</button>
				</div>
			)}

			<WorkspaceChangeNotice view={view} error={error} onRetry={refetch} />

			{/* Diff content -- virtualized; keep mounted during soft refresh */}
			<div className="relative min-h-0 flex-1">
				{isRefreshing ? (
					<div className="pointer-events-none absolute inset-x-0 top-0 z-10 flex justify-center pt-2">
						<div className="flex items-center gap-1.5 rounded-full border border-border/60 bg-background/90 px-2.5 py-1 text-[11px] text-muted-foreground shadow-sm backdrop-blur-sm">
							<Loader2Icon className="size-3 animate-spin" />
							Refreshing…
						</div>
					</div>
				) : null}
				{isInitialLoading ? (
					<div className="flex items-center justify-center gap-2 py-16">
						<Loader2Icon className="size-3.5 animate-spin text-muted-foreground/70" />
						<span className="text-[12px] text-muted-foreground">Loading…</span>
					</div>
				) : diffs.length === 0 ? (
					<EmptyState scope={scope} view={view} error={error} />
				) : (
					<VirtualizedDiffList
						diffs={displayedDiffs}
						diffStyle={settings.diffStyle}
						wordWrap={wordWrap}
						ignoreWhitespace={ignoreWhitespace}
						loadError={error}
						getIsCollapsed={getIsCollapsed}
						onToggle={toggleFile}
					/>
				)}
			</div>

			{/* Commit & push */}
			{canCommit && (
				<CommitPushDialog
					open={commitOpen}
					onOpenChange={setCommitOpen}
					directory={directory}
					onCommitted={handleCommitted}
				/>
			)}
		</div>
	)
})

function ScopeDropdown({
	scope,
	additions,
	deletions,
	scopeStats,
	onScopeChange,
}: {
	scope: WorkspaceChangeScope
	additions: number
	deletions: number
	scopeStats: Partial<Record<WorkspaceChangeScope, { additions: number; deletions: number }>>
	onScopeChange: (scope: WorkspaceChangeScope) => void
}) {
	const primaryScopes = WORKSPACE_CHANGE_SCOPES.filter((item) => item.scope !== "branch")
	const branchStats = scopeStats.branch
	return (
		<DropdownMenu>
			<DropdownMenuTrigger
				render={
					<button
						type="button"
						className="flex max-w-full min-w-0 items-center gap-1 rounded-md px-1 py-0.5 text-left text-[12px] font-medium text-foreground transition-colors hover:bg-muted/60"
						aria-label="Change scope"
					>
						<span className="truncate">{scopeLabel(scope)}</span>
						{additions > 0 || deletions > 0 ? (
							<span className="flex shrink-0 items-center gap-1 tabular-nums">
								{additions > 0 ? (
									<span className="text-emerald-600 dark:text-emerald-400">+{additions}</span>
								) : null}
								{deletions > 0 ? <span className="text-red-500/90">−{deletions}</span> : null}
							</span>
						) : null}
						<ChevronDownIcon className="size-3 shrink-0 text-muted-foreground" aria-hidden="true" />
					</button>
				}
			/>
			<DropdownMenuContent align="start" className="min-w-52">
				<DropdownMenuGroup>
					{primaryScopes.map((item) => {
						const active = item.scope === scope
						const preview = scopeStats[item.scope]
						return (
							<DropdownMenuItem
								key={item.scope}
								className="text-xs"
								onClick={() => onScopeChange(item.scope)}
							>
								<span className="flex-1">{item.label}</span>
								{preview && (preview.additions > 0 || preview.deletions > 0) ? (
									<span className="mr-1.5 flex shrink-0 items-center gap-1 tabular-nums text-muted-foreground">
										{preview.additions > 0 ? (
											<span className="text-emerald-600 dark:text-emerald-400">
												+{preview.additions}
											</span>
										) : null}
										{preview.deletions > 0 ? (
											<span className="text-red-500/90">−{preview.deletions}</span>
										) : null}
									</span>
								) : null}
								{active ? <CheckIcon className="size-3.5 text-foreground" /> : <span className="size-3.5" />}
							</DropdownMenuItem>
						)
					})}
				</DropdownMenuGroup>
				<DropdownMenuSeparator />
				<DropdownMenuGroup>
					<DropdownMenuItem className="text-xs" onClick={() => onScopeChange("branch")}>
						<span className="flex-1">Branch</span>
						{branchStats && (branchStats.additions > 0 || branchStats.deletions > 0) ? (
							<span className="mr-1.5 flex shrink-0 items-center gap-1 tabular-nums text-muted-foreground">
								{branchStats.additions > 0 ? (
									<span className="text-emerald-600 dark:text-emerald-400">
										+{branchStats.additions}
									</span>
								) : null}
								{branchStats.deletions > 0 ? (
									<span className="text-red-500/90">−{branchStats.deletions}</span>
								) : null}
							</span>
						) : null}
						{scope === "branch" ? (
							<CheckIcon className="size-3.5 text-foreground" />
						) : (
							<span className="size-3.5" />
						)}
					</DropdownMenuItem>
				</DropdownMenuGroup>
			</DropdownMenuContent>
		</DropdownMenu>
	)
}

function BranchDropdown({
	directory,
	currentBranch,
	baseBranch,
	scope,
	onSelectBranch,
}: {
	directory: string
	currentBranch: string
	baseBranch: string | null
	scope: WorkspaceChangeScope
	onSelectBranch: (branch: string) => void
}) {
	const [branches, setBranches] = useState<string[]>([])
	const [loading, setLoading] = useState(false)
	const label = scope === "branch" && baseBranch ? baseBranch : currentBranch

	const loadBranches = useCallback(async () => {
		if (!directory || !isElectron) return
		setLoading(true)
		try {
			const info = await fetchGitBranches(directory)
			const locals = info.local.length > 0 ? info.local : [info.current].filter(Boolean)
			setBranches(locals)
		} catch {
			setBranches([currentBranch].filter(Boolean))
		} finally {
			setLoading(false)
		}
	}, [currentBranch, directory])

	return (
		<DropdownMenu
			onOpenChange={(open) => {
				if (open) void loadBranches()
			}}
		>
			<DropdownMenuTrigger
				render={
					<button
						type="button"
						className="flex max-w-[42%] min-w-0 items-center gap-1 rounded-md px-1 py-0.5 text-left text-[12px] text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
						title={label}
						aria-label="Compare branch"
					>
						<span className="truncate">{label}</span>
						<ChevronDownIcon className="size-3 shrink-0 opacity-70" aria-hidden="true" />
					</button>
				}
			/>
			<DropdownMenuContent align="start" className="min-w-48 max-w-72">
				{loading ? (
					<div className="flex items-center gap-2 px-2 py-1.5 text-xs text-muted-foreground">
						<Loader2Icon className="size-3 animate-spin" />
						Loading branches…
					</div>
				) : branches.length === 0 ? (
					<div className="px-2 py-1.5 text-xs text-muted-foreground">No branches found</div>
				) : (
					<DropdownMenuGroup>
						{branches.map((branch) => {
							const active = scope === "branch" && (baseBranch ?? currentBranch) === branch
							return (
								<DropdownMenuItem
									key={branch}
									className="text-xs"
									onClick={() => onSelectBranch(branch)}
								>
									<span className="min-w-0 flex-1 truncate">{branch}</span>
									{active ? <CheckIcon className="size-3.5 shrink-0 text-foreground" /> : null}
								</DropdownMenuItem>
							)
						})}
					</DropdownMenuGroup>
				)}
			</DropdownMenuContent>
		</DropdownMenu>
	)
}

interface OptionsMenuProps {
	settings: {
		diffStyle: DiffStyle
		ignoreWhitespace?: boolean
		wordWrap?: boolean
	}
	onSetDiffStyle: (style: DiffStyle) => void
	onToggleIgnoreWhitespace: () => void
	onToggleWordWrap: () => void
	onFind: () => void
	onToggleCollapseExpandAll: () => void
	allCollapsed: boolean
	onRefresh: () => void
	hasChanges: boolean
}

function OptionsMenu({
	settings,
	onSetDiffStyle,
	onToggleIgnoreWhitespace,
	onToggleWordWrap,
	onFind,
	onToggleCollapseExpandAll,
	allCollapsed,
	onRefresh,
	hasChanges,
}: OptionsMenuProps) {
	return (
		<DropdownMenu>
			<DropdownMenuTrigger
				render={
					<button
						type="button"
						className="rounded-md p-1 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
						title="Changes options"
						aria-label="Changes options"
					>
						<EllipsisIcon className="size-3.5" />
					</button>
				}
			/>
			<DropdownMenuContent align="end" className="min-w-48">
				<DropdownMenuGroup>
					<DropdownMenuLabel>Layout</DropdownMenuLabel>
					<DropdownMenuRadioGroup
						value={settings.diffStyle ?? "unified"}
						onValueChange={(detail) => onSetDiffStyle(detail as DiffStyle)}
					>
						<DropdownMenuRadioItem value="unified" className="text-xs">
							Unified
						</DropdownMenuRadioItem>
						<DropdownMenuRadioItem value="split" className="text-xs">
							Split
						</DropdownMenuRadioItem>
					</DropdownMenuRadioGroup>
				</DropdownMenuGroup>
				<DropdownMenuSeparator />
				<DropdownMenuGroup>
					<DropdownMenuCheckboxItem
						checked={settings.ignoreWhitespace ?? false}
						onClick={onToggleIgnoreWhitespace}
						closeOnClick={false}
						className="text-xs"
					>
						Ignore whitespace
					</DropdownMenuCheckboxItem>
					<DropdownMenuCheckboxItem
						checked={settings.wordWrap ?? false}
						onClick={onToggleWordWrap}
						closeOnClick={false}
						className="text-xs"
					>
						Word wrap
					</DropdownMenuCheckboxItem>
				</DropdownMenuGroup>
				<DropdownMenuSeparator />
				<DropdownMenuGroup>
					<DropdownMenuItem onClick={onFind} className="text-xs">
						<SearchIcon /> Find in changes
					</DropdownMenuItem>
					<DropdownMenuItem
						onClick={onToggleCollapseExpandAll}
						disabled={!hasChanges}
						className="text-xs"
					>
						{allCollapsed ? (
							<>
								<ChevronsUpDownIcon /> Expand all
							</>
						) : (
							<>
								<ChevronsDownUpIcon /> Collapse all
							</>
						)}
					</DropdownMenuItem>
					<DropdownMenuItem onClick={onRefresh} className="text-xs">
						<RefreshCwIcon /> Refresh changes
					</DropdownMenuItem>
				</DropdownMenuGroup>
			</DropdownMenuContent>
		</DropdownMenu>
	)
}

function WorkspaceChangeNotice({
	view,
	error,
	onRetry,
}: {
	view: WorkspaceChangeView | null
	error: string | null
	onRetry?: () => void
}) {
	const warnings = view?.warnings ?? []
	if (!error && warnings.length === 0 && view?.status !== "partial") return null
	const timedOut = Boolean(error?.toLowerCase().includes("timed out"))
	return (
		<div className="border-b border-border/60 bg-muted/15 px-3 py-2.5 text-[12px] text-muted-foreground">
			{error ? (
				<div className="flex items-start gap-2">
					<AlertTriangleIcon className="mt-0.5 size-3.5 shrink-0 text-red-500/80" />
					<div className="min-w-0 flex-1 space-y-1">
						<p className="text-red-500/90">{timedOut ? "Changes took too long to load" : error}</p>
						{timedOut ? (
							<p className="text-[11px] text-muted-foreground">
								Large git trees can be slow. Try All / Staged, or refresh.
							</p>
						) : null}
						{onRetry ? (
							<button
								type="button"
								onClick={onRetry}
								className="text-[11px] font-medium text-foreground underline-offset-2 hover:underline"
							>
								Retry
							</button>
						) : null}
					</div>
				</div>
			) : (
				<div className="flex flex-wrap items-center gap-1.5">
					<AlertTriangleIcon className="size-3.5 text-amber-500/80" />
					<span className="text-amber-600 dark:text-amber-400">Partial change view</span>
					{warnings.map((warning) => (
						<span key={warning} className="rounded bg-muted px-1.5 py-0.5 text-[11px]">
							{warning}
						</span>
					))}
				</div>
			)}
		</div>
	)
}

// ============================================================
// Virtualized diff list using TanStack Virtual
// ============================================================

interface VirtualizedDiffListProps {
	diffs: WorkspacePatchFile[]
	diffStyle: DiffStyle
	wordWrap: boolean
	ignoreWhitespace: boolean
	loadError?: string | null
	getIsCollapsed: (diff: WorkspacePatchFile) => boolean
	onToggle: (file: string) => void
}

const VirtualizedDiffList = memo(function VirtualizedDiffList({
	diffs,
	diffStyle,
	wordWrap,
	ignoreWhitespace,
	loadError,
	getIsCollapsed,
	onToggle,
}: VirtualizedDiffListProps) {
	const scrollRef = useRef<HTMLDivElement>(null)
	const isDark = useIsDarkMode()
	const [pinnedDiff, setPinnedDiff] = useState<WorkspacePatchFile | null>(null)
	const pinnedFileRef = useRef<string | null>(null)

	const theme = isDark ? ("one-dark-pro" as const) : ("one-light" as const)

	// Stable highlighter options for the worker pool (theme + lineDiffType).
	// When using the worker pool, these are controlled by the pool, not per-component.
	const highlighterOptions = useMemo(
		() => ({
			theme,
			lineDiffType: "word" as const,
		}),
		[theme],
	)

	const virtualizer = useVirtualizer({
		count: diffs.length,
		getScrollElement: () => scrollRef.current,
		estimateSize: (index) => {
			const diff = diffs[index]
			if (getIsCollapsed(diff)) return COLLAPSED_ROW_HEIGHT
			if (isLargeDiff(diff)) return COLLAPSED_ROW_HEIGHT + 80 // header + placeholder
			// Rough estimate based on line count (collapsed unchanged hunks help)
			const lines = Math.min(diff.additions + diff.deletions, 200)
			return COLLAPSED_ROW_HEIGHT + lines * 20
		},
		overscan: 3,
	})

	// --- Pinned header: detect which expanded file's header has scrolled past ---
	useEffect(() => {
		const el = scrollRef.current
		if (!el) return
		let rafId: number | null = null

		const onScroll = () => {
			if (rafId !== null) return
			rafId = requestAnimationFrame(() => {
				rafId = null
				const scrollTop = el.scrollTop
				// Near the top -- nothing to pin
				if (scrollTop < COLLAPSED_ROW_HEIGHT) {
					if (pinnedFileRef.current !== null) {
						pinnedFileRef.current = null
						setPinnedDiff(null)
					}
					return
				}
				// Find the expanded file whose header has fully scrolled out of view
				// but whose body still extends below the viewport top
				let found: WorkspacePatchFile | null = null
				for (const item of virtualizer.getVirtualItems()) {
					const diff = diffs[item.index]
					if (getIsCollapsed(diff)) continue
					const headerBottom = item.start + COLLAPSED_ROW_HEIGHT
					if (
						headerBottom <= scrollTop &&
						item.start + item.size > scrollTop + COLLAPSED_ROW_HEIGHT
					) {
						found = diff
						break
					}
				}
				const foundFile = found?.file ?? null
				if (pinnedFileRef.current !== foundFile) {
					pinnedFileRef.current = foundFile
					setPinnedDiff(found)
				}
			})
		}

		el.addEventListener("scroll", onScroll, { passive: true })
		return () => {
			el.removeEventListener("scroll", onScroll)
			if (rafId !== null) cancelAnimationFrame(rafId)
		}
	}, [diffs, getIsCollapsed]) // virtualizer accessed via closure, always current

	const handlePinnedToggle = useCallback(() => {
		if (pinnedDiff) onToggle(pinnedDiff.file)
	}, [pinnedDiff, onToggle])

	return (
		<WorkerPoolContextProvider
			poolOptions={WORKER_POOL_OPTIONS}
			highlighterOptions={highlighterOptions}
		>
			<DiffThemeSyncer />
			<div className="relative h-full">
				{/* Pinned file header -- shown when an expanded file's header scrolls past */}
				{pinnedDiff && (
					<div
						key={pinnedDiff.file}
						className="absolute inset-x-0 top-0 z-10 animate-in fade-in duration-150 border-b border-border/50 bg-background/60 shadow-sm backdrop-blur-md"
					>
						<FileDiffHeader
							file={pinnedDiff.file}
							additions={pinnedDiff.additions}
							deletions={pinnedDiff.deletions}
							collapsed={false}
							onToggle={handlePinnedToggle}
							isGenerated={isGeneratedFile(pinnedDiff.file)}
						/>
					</div>
				)}
				<div ref={scrollRef} className="h-full overflow-auto">
					<div
						style={{
							height: `${virtualizer.getTotalSize()}px`,
							width: "100%",
							position: "relative",
						}}
					>
						{virtualizer.getVirtualItems().map((virtualRow) => {
							const diff = diffs[virtualRow.index]
							const collapsed = getIsCollapsed(diff)
							return (
								<div
									key={diff.file}
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
									<FileDiffSection
										diff={diff}
										diffStyle={diffStyle}
										wordWrap={wordWrap}
										ignoreWhitespace={ignoreWhitespace}
										collapsed={collapsed}
										loadError={loadError}
										onToggle={onToggle}
									/>
								</div>
							)
						})}
					</div>
				</div>
			</div>
		</WorkerPoolContextProvider>
	)
})

// ============================================================
// Worker pool theme syncer
// ============================================================

/**
 * Tiny component that syncs the active theme to the worker pool when it changes.
 * Lives inside WorkerPoolContextProvider so it can call useWorkerPool().
 */
function DiffThemeSyncer() {
	const pool = useWorkerPool()
	const isDark = useIsDarkMode()
	const prevThemeRef = useRef<string | null>(null)

	useEffect(() => {
		if (!pool) return
		const theme = isDark ? "one-dark-pro" : "one-light"
		if (prevThemeRef.current === theme) return
		prevThemeRef.current = theme
		pool.setRenderOptions({ theme })
	}, [pool, isDark])

	return null
}

// ============================================================
// Per-file diff section
// ============================================================

interface FileDiffSectionProps {
	diff: WorkspacePatchFile
	diffStyle: DiffStyle
	wordWrap: boolean
	ignoreWhitespace: boolean
	collapsed: boolean
	loadError?: string | null
	onToggle: (file: string) => void
}

const FileDiffSection = memo(function FileDiffSection({
	diff,
	diffStyle,
	wordWrap,
	ignoreWhitespace,
	collapsed,
	loadError,
	onToggle,
}: FileDiffSectionProps) {
	const generated = isGeneratedFile(diff.file)
	const large = isLargeDiff(diff)
	const [loadLargeDiff, setLoadLargeDiff] = useState(!large)

	const fileName = diff.file.split(/[/\\]/).pop() || diff.file
	const hasSides = diff.oldText != null || diff.newText != null
	const gitFileDiff = useMemo(() => {
		if (!diff.patch || !hasSides) return null
		return buildExpandableGitFileDiff({
			patch: diff.patch,
			fileName,
			oldText: diff.oldText,
			newText: diff.newText,
		})
	}, [diff.newText, diff.oldText, diff.patch, fileName, hasSides])

	// Per-component options (only non-pool-controlled settings).
	// theme and lineDiffType are managed by the WorkerPoolManager.
	const options = useMemo(
		() => ({
			diffStyle: diffStyle as "unified" | "split",
			disableFileHeader: true,
			expandUnchanged: false,
			overflow: (wordWrap ? "wrap" : "scroll") as "wrap" | "scroll",
			unsafeCSS: REVIEW_DIFF_UNSAFE_CSS,
			parseDiffOptions: {
				stripTrailingCr: true,
				ignoreWhitespace,
			},
		}),
		[diffStyle, ignoreWhitespace, wordWrap],
	)

	const handleToggle = useCallback(() => onToggle(diff.file), [diff.file, onToggle])
	const handleLoadLarge = useCallback(() => {
		startTransition(() => setLoadLargeDiff(true))
	}, [])

	// Determine what body content to show
	let body: ReactNode = null
	if (!collapsed) {
		if (!loadLargeDiff) {
			body = (
				<LargeDiffPlaceholder
					additions={diff.additions}
					deletions={diff.deletions}
					onLoad={handleLoadLarge}
				/>
			)
		} else {
			// Prefer git patch + sides (hunks match numstat, expand still works).
			// Fall back to MultiFileDiff (EOL/whitespace normalized) or PatchDiff.
			body = (
				<div className={cn(wordWrap ? "overflow-x-hidden" : "overflow-x-auto", "overflow-y-hidden")}>
					{gitFileDiff ? (
						<FileDiff options={options} fileDiff={gitFileDiff} />
					) : hasSides ? (
						<MultiFileDiff
							options={options}
							oldFile={{ name: fileName, contents: diff.oldText ?? "" }}
							newFile={{ name: fileName, contents: diff.newText ?? "" }}
						/>
					) : diff.patch ? (
						<PatchDiff options={options} patch={diff.patch} />
					) : diff.patchPending && !loadError ? (
						<div className="flex items-center gap-1.5 bg-muted/10 px-4 py-5 text-xs text-muted-foreground">
							<Loader2Icon className="size-3.5 animate-spin" />
							<span>Loading diff…</span>
						</div>
					) : (
						<MetadataOnlyPlaceholder warnings={diff.warnings} />
					)}
				</div>
			)
		}
	}

	return (
		<div className="border-b border-border last:border-b-0">
			<FileDiffHeader
				file={diff.file}
				additions={diff.additions}
				deletions={diff.deletions}
				collapsed={collapsed}
				onToggle={handleToggle}
				isLarge={large && !loadLargeDiff}
				isGenerated={generated}
			/>
			{body}
		</div>
	)
})

// ============================================================
// Large diff placeholder
// ============================================================

function LargeDiffPlaceholder({
	additions,
	deletions,
	onLoad,
}: {
	additions: number
	deletions: number
	onLoad: () => void
}) {
	const totalLines = additions + deletions
	return (
		<div className="flex flex-col items-center justify-center gap-2 bg-muted/10 px-4 py-6">
			<div className="flex items-center gap-1.5 text-xs text-amber-500">
				<AlertTriangleIcon className="size-3.5" />
				<span>Large diff ({totalLines.toLocaleString()} lines changed) not shown</span>
			</div>
			<button
				type="button"
				onClick={onLoad}
				className="rounded-md border border-border bg-background px-3 py-1 text-xs text-foreground transition-colors hover:bg-muted"
			>
				Load diff
			</button>
		</div>
	)
}

function MetadataOnlyPlaceholder({ warnings }: { warnings: string[] }) {
	return (
		<div className="flex flex-col gap-1.5 bg-muted/10 px-4 py-5 text-xs text-muted-foreground">
			<div className="flex items-center gap-1.5 text-amber-500">
				<AlertTriangleIcon className="size-3.5" />
				<span>Text diff is not available for this file</span>
			</div>
			{warnings.length > 0 && (
				<div className="flex flex-wrap gap-1">
					{warnings.map((warning) => (
						<span key={warning} className="rounded bg-muted px-1.5 py-0.5">
							{warning}
						</span>
					))}
				</div>
			)}
		</div>
	)
}

// ============================================================
// File diff header
// ============================================================

const FileDiffHeader = memo(function FileDiffHeader({
	file,
	additions,
	deletions,
	collapsed,
	onToggle,
	isLarge,
	isGenerated,
	loading,
}: {
	file: string
	additions: number
	deletions: number
	collapsed: boolean
	onToggle: () => void
	isLarge?: boolean
	isGenerated?: boolean
	loading?: boolean
}) {
	return (
		<button
			type="button"
			onClick={onToggle}
			className="flex w-full items-center gap-2 bg-muted/30 px-3 py-1.5 text-left transition-colors hover:bg-muted/50"
		>
			{loading ? (
				<Loader2Icon className="size-3 shrink-0 animate-spin text-muted-foreground" />
			) : collapsed ? (
				<ChevronRightIcon className="size-3 shrink-0 text-muted-foreground" />
			) : (
				<ChevronDownIcon className="size-3 shrink-0 text-muted-foreground" />
			)}
			<span
				className={cn(
					"min-w-0 flex-1 truncate font-mono text-xs",
					isGenerated ? "italic text-muted-foreground" : "text-foreground",
				)}
			>
				{file}
			</span>
			<span className="flex shrink-0 items-center gap-1.5 text-[11px]">
				{isGenerated && (
					<span className="rounded bg-muted px-1 py-0.5 text-[9px] font-medium leading-none text-muted-foreground/60">
						generated
					</span>
				)}
				{isLarge && (
					<span className="rounded bg-amber-500/15 px-1 py-0.5 text-[9px] font-medium leading-none text-amber-500">
						LARGE
					</span>
				)}
				{additions > 0 || deletions > 0 ? (
					<>
						<span className="flex items-center gap-0.5 text-green-500">
							<PlusIcon className="size-2.5" aria-hidden="true" />
							{additions}
						</span>
						<span className="flex items-center gap-0.5 text-red-500">
							<MinusIcon className="size-2.5" aria-hidden="true" />
							{deletions}
						</span>
					</>
				) : null}
			</span>
		</button>
	)
})

// ============================================================
// Empty state
// ============================================================

function EmptyState({
	scope,
	view,
	error,
}: {
	scope: WorkspaceChangeScope
	view: WorkspaceChangeView | null
	error: string | null
}) {
	const label = scopeLabel(scope)
	const timedOut = Boolean(error?.toLowerCase().includes("timed out"))
	const title = error
		? timedOut
			? "Changes took too long"
			: "Unable to load changes"
		: view?.status === "unsupported"
			? `${label} unavailable`
			: "No changes"
	const detail = error
		? timedOut
			? "Try Uncommitted or Staged, or refresh."
			: error
		: view?.status === "unsupported"
			? (view.warnings[0] ?? "This workspace does not support that change scope")
			: "Edits will show up here as the agent works."
	return (
		<div className="flex flex-col items-center justify-center gap-2 px-6 py-20">
			<p className="text-[13px] font-medium text-foreground/90">{title}</p>
			<p className="max-w-[220px] text-center text-[12px] leading-relaxed text-muted-foreground">
				{detail}
			</p>
		</div>
	)
}
