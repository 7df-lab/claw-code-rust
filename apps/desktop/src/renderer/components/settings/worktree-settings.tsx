/**
 * Worktree management settings page.
 *
 * Lists all worktrees across connected projects using the Devo worktree API.
 * Provides remove and reset actions for each worktree.
 *
 * To avoid excessive API requests, the fetch is gated on a stable "directory key"
 * derived from the project list. The full `projectListAtom` includes volatile
 * fields (`agentCount`, `lastActiveAt`) that change on every session update;
 * without the key the effect would refire on each update, sending N requests
 * per project per re-render.
 */

import { Button } from "@devo/ui/components/button"
import { GitForkIcon, Loader2Icon, RotateCcwIcon, TrashIcon } from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useProjectList } from "../../hooks/use-agents"
import { listWorktrees, removeWorktree, resetWorktree } from "../../services/worktree-service"
import { SettingsSection } from "./settings-section"

// ============================================================
// Types
// ============================================================

interface WorktreeEntry {
	/** The worktree directory path */
	directory: string
	/** The project directory this worktree belongs to */
	projectDir: string
	/** Human-readable project name */
	projectName: string
}

// ============================================================
// Helpers
// ============================================================

/** Extracts the last path segment as a display name */
function dirName(dir: string): string {
	return dir.split("/").filter(Boolean).pop() ?? dir
}

// ============================================================
// Main component
// ============================================================

export function WorktreeSettings() {
	const projects = useProjectList()
	const [worktrees, setWorktrees] = useState<WorktreeEntry[]>([])
	const [loading, setLoading] = useState(true)
	const [removing, setRemoving] = useState<string | null>(null)
	const [resetting, setResetting] = useState<string | null>(null)

	// Keep a ref so the stable callback always reads the latest project list
	// without needing it as a dependency.
	const projectsRef = useRef(projects)
	projectsRef.current = projects

	// Stable key: only changes when the set of project directories changes,
	// NOT when volatile fields like agentCount or lastActiveAt update.
	const projectDirKey = useMemo(
		() => projects.map((p) => p.directory).sort().join("\0"),
		[projects],
	)

	const loadWorktrees = useCallback(async () => {
		const currentProjects = projectsRef.current
		setLoading(true)
		try {
			const results = await Promise.allSettled(
				currentProjects.map(async (project) => {
					const dirs = await listWorktrees(project.directory)
					return dirs.map(
						(dir): WorktreeEntry => ({
							directory: dir,
							projectDir: project.directory,
							projectName: project.name,
						}),
					)
				}),
			)

			const entries: WorktreeEntry[] = []
			for (const result of results) {
				if (result.status === "fulfilled") {
					entries.push(...result.value)
				}
			}
			setWorktrees(entries)
		} catch {
			// Silently fail
		} finally {
			setLoading(false)
		}
	}, [])

	// Only refetch when the set of project directories actually changes (or on mount).
	useEffect(() => {
		loadWorktrees()
	}, [projectDirKey, loadWorktrees])

	const handleRemove = useCallback(
		async (wt: WorktreeEntry) => {
			setRemoving(wt.directory)
			try {
				await removeWorktree(wt.projectDir, wt.directory)
				await loadWorktrees()
			} catch {
				// Silently fail
			} finally {
				setRemoving(null)
			}
		},
		[loadWorktrees],
	)

	const handleReset = useCallback(
		async (wt: WorktreeEntry) => {
			setResetting(wt.directory)
			try {
				await resetWorktree(wt.projectDir, wt.directory)
				await loadWorktrees()
			} catch {
				// Silently fail
			} finally {
				setResetting(null)
			}
		},
		[loadWorktrees],
	)

	// Use the project count from the stable ref for the summary display,
	// so we don't need the volatile `projects` array in the render path.
	const projectCount = projects.length

	return (
		<div className="space-y-8">
			<div>
				<h2 className="text-[22px] font-medium tracking-tight">Worktrees</h2>
				<p className="mt-1 text-sm text-muted-foreground">
					Manage git worktrees created for isolated agent sessions.
				</p>
			</div>

			{/* Summary */}
			<SettingsSection title="Overview">
				<div className="flex items-center gap-2 px-4 py-3">
					<GitForkIcon className="size-4 text-muted-foreground" aria-hidden="true" />
					<span className="text-sm">
						{worktrees.length} worktree{worktrees.length !== 1 ? "s" : ""}
						{projectCount > 0 && (
							<span className="text-muted-foreground">
								{" "}
								across {projectCount} project{projectCount !== 1 ? "s" : ""}
							</span>
						)}
					</span>
				</div>
			</SettingsSection>

			{/* Worktree list */}
			{loading ? (
				<div className="flex items-center justify-center py-8">
					<Loader2Icon className="size-5 animate-spin text-muted-foreground" />
				</div>
			) : worktrees.length === 0 ? (
				<div className="rounded-lg border border-dashed border-border py-8 text-center">
					<GitForkIcon className="mx-auto size-8 text-muted-foreground/30" aria-hidden="true" />
					<p className="mt-2 text-sm text-muted-foreground">No worktrees</p>
					<p className="text-xs text-muted-foreground/60">
						Worktrees will appear here when you create sessions in worktree mode.
					</p>
				</div>
			) : (
				<SettingsSection title="Active Worktrees">
					{worktrees.map((wt) => (
						<WorktreeRow
							key={wt.directory}
							worktree={wt}
							isRemoving={removing === wt.directory}
							isResetting={resetting === wt.directory}
							onRemove={() => handleRemove(wt)}
							onReset={() => handleReset(wt)}
						/>
					))}
				</SettingsSection>
			)}
		</div>
	)
}

// ============================================================
// Sub-components
// ============================================================

function WorktreeRow({
	worktree,
	isRemoving,
	isResetting,
	onRemove,
	onReset,
}: {
	worktree: WorktreeEntry
	isRemoving: boolean
	isResetting: boolean
	onRemove: () => void
	onReset: () => void
}) {
	return (
		<div className="flex items-center gap-3 px-4 py-3">
			<GitForkIcon className="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />

			<div className="min-w-0 flex-1">
				<div className="flex items-center gap-2">
					<span className="truncate text-sm font-medium">{dirName(worktree.directory)}</span>
				</div>
				<div className="flex items-center gap-2 text-xs text-muted-foreground/60">
					<span>{worktree.projectName}</span>
					<span>-</span>
					<span className="truncate">{worktree.directory}</span>
				</div>
			</div>

			<Button
				size="sm"
				variant="ghost"
				onClick={onReset}
				disabled={isResetting || isRemoving}
				className="h-7 w-7 shrink-0 p-0 text-muted-foreground hover:text-foreground"
				title="Reset worktree to default branch"
			>
				{isResetting ? (
					<Loader2Icon className="size-3.5 animate-spin" />
				) : (
					<RotateCcwIcon className="size-3.5" />
				)}
			</Button>

			<Button
				size="sm"
				variant="ghost"
				onClick={onRemove}
				disabled={isRemoving || isResetting}
				className="h-7 w-7 shrink-0 p-0 text-muted-foreground hover:text-red-500"
				title="Remove worktree"
			>
				{isRemoving ? (
					<Loader2Icon className="size-3.5 animate-spin" />
				) : (
					<TrashIcon className="size-3.5" />
				)}
			</Button>
		</div>
	)
}
