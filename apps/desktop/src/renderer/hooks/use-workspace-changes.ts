import type { WorkspaceChangeScope, WorkspaceChangeView } from "@devo-ai/sdk/v2/client"
import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useEffect, useMemo, useRef } from "react"
import { isMockModeAtom } from "../atoms/mock-mode"
import { appStore } from "../atoms/store"
import {
	latestWorkspaceTurnIdFamily,
	markWorkspaceChangesLoadingAtom,
	registerSessionWorkspaceDirectoryAtom,
	setWorkspaceChangesErrorAtom,
	setWorkspaceChangesViewAtom,
	workspaceChangesKey,
	workspaceChangesStateFamily,
} from "../atoms/workspace-changes"
import { numberFromProtocol, isCompletePatch } from "../lib/workspace-diff"
import { getProjectClient } from "../services/connection-manager"
import { getWorkspaceChanges } from "../services/devo"

const FULL_DIFF_LIMIT_BYTES = 2_000_000

/** Sibling scopes warmed after the active Summary paints. */
const PREFETCH_SCOPES: WorkspaceChangeScope[] = [
	"uncommitted",
	"unstaged",
	"staged",
	"branch",
]

export type ScopeStatsPreview = {
	additions: number
	deletions: number
}

type FetchArgs = {
	sessionId: string
	directory: string
	scope: WorkspaceChangeScope
	baseBranch?: string | null
	turnId?: string | null
	ignoreWhitespace: boolean
}

function cacheKeyFor(args: {
	sessionId: string
	directory: string
	scope: WorkspaceChangeScope
	turnId?: string | null
	baseBranch?: string | null
	ignoreWhitespace: boolean
}): string {
	return workspaceChangesKey({
		cwd: args.directory,
		sessionId: args.sessionId,
		scope: args.scope,
		turnId: args.turnId ?? undefined,
		baseBranch: args.baseBranch,
		ignoreWhitespace: args.ignoreWhitespace,
	})
}

function needsSummary(key: string): boolean {
	const state = appStore.get(workspaceChangesStateFamily(key))
	return !state.view || state.stale
}

function hasFilePatch(view: WorkspaceChangeView, filePath: string): boolean {
	const normalized = filePath.replace(/\\/g, "/")
	const existing = view.unified_diff ?? ""
	if (
		!(
			existing.includes(`b/${normalized}\n`) ||
			existing.includes(`b/${normalized}\r\n`) ||
			existing.endsWith(`b/${normalized}`)
		)
	) {
		return false
	}
	// Header-only stubs include `b/path` but are not complete patches.
	const chunk = extractFilePatch(existing, normalized)
	return isCompletePatch(chunk)
}

function extractFilePatch(diff: string, normalizedPath: string): string | null {
	const parts = diff.split(/(?=^diff --git )/m)
	for (const part of parts) {
		if (
			part.includes(`b/${normalizedPath}\n`) ||
			part.includes(`b/${normalizedPath}\r\n`) ||
			part.trimEnd().endsWith(`b/${normalizedPath}`)
		) {
			return part
		}
	}
	return null
}

function stripFilePatch(diff: string, filePath: string): string {
	const normalized = filePath.replace(/\\/g, "/")
	const parts = diff.split(/(?=^diff --git )/m)
	return parts
		.filter((part) => {
			const trimmed = part.trimStart()
			if (!trimmed) return false
			return !(
				trimmed.includes(`b/${normalized}\n`) ||
				trimmed.includes(`b/${normalized}\r\n`) ||
				trimmed.trimEnd().endsWith(`b/${normalized}`)
			)
		})
		.join("")
		.replace(/^\n+/, "")
}

/** True when every non-binary file already has a complete patch chunk. */
function viewHasCompletePatches(view: WorkspaceChangeView): boolean {
	if (view.files.length === 0) return Boolean(view.unified_diff != null)
	const diff = view.unified_diff
	if (diff == null || diff.length === 0) return false
	return view.files.every((file) => Boolean(file.binary) || hasFilePatch(view, String(file.path)))
}

function findFile(view: WorkspaceChangeView, filePath: string) {
	const normalized = filePath.replace(/\\/g, "/")
	return view.files.find((file) => String(file.path).replace(/\\/g, "/") === normalized)
}

function fileHasSides(view: WorkspaceChangeView, filePath: string): boolean {
	const file = findFile(view, filePath)
	if (!file) return false
	return typeof file.old_text === "string" || typeof file.new_text === "string"
}

/** Ready for MultiFileDiff, or PatchDiff/binary fallback without another sides fetch. */
function fileExpandReady(
	view: WorkspaceChangeView,
	filePath: string,
	sidesFetched: Set<string>,
	cacheKey: string,
): boolean {
	const normalized = filePath.replace(/\\/g, "/")
	const file = findFile(view, filePath)
	if (!file) return false
	if (file.binary) return true
	if (fileHasSides(view, filePath)) return true
	const sideKey = `${cacheKey}\u001f${normalized}`
	// Path-scoped sides already attempted (oversize/unavailable) — stop refetching.
	if (sidesFetched.has(sideKey)) return true
	return false
}

export function mergePathFullIntoView(
	base: WorkspaceChangeView,
	patchView: WorkspaceChangeView,
): WorkspaceChangeView {
	const files = base.files.map((file) => {
		const path = String(file.path).replace(/\\/g, "/")
		const updated = patchView.files.find((item) => String(item.path).replace(/\\/g, "/") === path)
		if (!updated) return file
		return {
			...file,
			status: updated.status ?? file.status,
			additions: updated.additions ?? file.additions,
			deletions: updated.deletions ?? file.deletions,
			binary: updated.binary ?? file.binary,
			diff_truncated: updated.diff_truncated ?? file.diff_truncated,
			old_text: updated.old_text ?? file.old_text,
			new_text: updated.new_text ?? file.new_text,
		}
	})
	// Keep Summary-only rows (e.g. untracked) that Full omitted.
	for (const incoming of patchView.files) {
		const path = String(incoming.path).replace(/\\/g, "/")
		if (files.some((file) => String(file.path).replace(/\\/g, "/") === path)) continue
		files.push(incoming)
	}
	const incoming = (patchView.unified_diff ?? "").trimEnd()
	const paths = patchView.files.map((file) => String(file.path).replace(/\\/g, "/"))
	if (!incoming) return { ...base, files }
	let baseDiff = base.unified_diff ?? ""
	for (const path of paths) {
		if (!hasFilePatch(base, path)) {
			baseDiff = stripFilePatch(baseDiff, path)
		}
	}
	const already = paths.every((path) => hasFilePatch(base, path))
	if (already) return { ...base, files }
	const unified = baseDiff.trimEnd() ? `${baseDiff.trimEnd()}\n${incoming}\n` : `${incoming}\n`
	return { ...base, files, unified_diff: unified }
}

/** Summary refreshes must not wipe expand-on-demand patches/sides. */
export function mergeSummaryPreservingExpandState(
	previous: WorkspaceChangeView | null | undefined,
	summary: WorkspaceChangeView,
): WorkspaceChangeView {
	if (!previous) return summary
	const prevByPath = new Map(
		previous.files.map((file) => [String(file.path).replace(/\\/g, "/"), file] as const),
	)
	const files = summary.files.map((file) => {
		const path = String(file.path).replace(/\\/g, "/")
		const prev = prevByPath.get(path)
		if (!prev) return file
		return {
			...file,
			old_text: prev.old_text ?? file.old_text,
			new_text: prev.new_text ?? file.new_text,
		}
	})
	let unified = previous.unified_diff ?? undefined
	if (unified) {
		const keptPaths = new Set(files.map((file) => String(file.path).replace(/\\/g, "/")))
		for (const prev of previous.files) {
			const path = String(prev.path).replace(/\\/g, "/")
			if (!keptPaths.has(path)) {
				unified = stripFilePatch(unified, path)
			}
		}
		if (!unified.trim()) unified = undefined
	}
	return {
		...summary,
		files,
		unified_diff: unified ?? summary.unified_diff,
	}
}

export function useWorkspaceChanges(
	sessionId: string,
	directory: string,
	scope: WorkspaceChangeScope,
	options: {
		enabled?: boolean
		baseBranch?: string | null
		ignoreWhitespace?: boolean
		prefetch?: boolean
	} = {},
) {
	const latestTurnId = useAtomValue(latestWorkspaceTurnIdFamily(sessionId))
	const turnId = scope === "turn" ? latestTurnId : undefined
	const ignoreWhitespace = options.ignoreWhitespace ?? false
	const key = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope,
				turnId,
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[sessionId, directory, scope, turnId, options.baseBranch, ignoreWhitespace],
	)
	const state = useAtomValue(workspaceChangesStateFamily(key))
	const fallbackKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope,
				turnId,
				baseBranch: options.baseBranch,
				ignoreWhitespace: !ignoreWhitespace,
			}),
		[sessionId, directory, scope, turnId, options.baseBranch, ignoreWhitespace],
	)
	const fallbackState = useAtomValue(workspaceChangesStateFamily(fallbackKey))
	const markLoading = useSetAtom(markWorkspaceChangesLoadingAtom)
	const setView = useSetAtom(setWorkspaceChangesViewAtom)
	const setError = useSetAtom(setWorkspaceChangesErrorAtom)
	const registerDirectory = useSetAtom(registerSessionWorkspaceDirectoryAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const enabled = options.enabled ?? true
	const prefetch = options.prefetch ?? true
	const inFlightRef = useRef(new Set<string>())
	const sidesFetchedRef = useRef(new Set<string>())

	useEffect(() => {
		sidesFetchedRef.current.clear()
	}, [key])

	useEffect(() => {
		if (sessionId && directory) {
			registerDirectory({ sessionId, directory })
		}
	}, [directory, registerDirectory, sessionId])

	const fetchServer = useCallback(
		async (args: FetchArgs, detail: "summary" | "full") => {
			if (isMockMode) return
			const client = getProjectClient(args.directory)
			const fetchKey = cacheKeyFor(args)
			const flightKey = `${fetchKey}\u001f${detail}`
			if (inFlightRef.current.has(flightKey)) return
			if (!client) {
				setError({
					key: fetchKey,
					error: args.directory
						? "No workspace connection for this project"
						: "Open a project to load changes",
				})
				return
			}
			inFlightRef.current.add(flightKey)
			const existing = appStore.get(workspaceChangesStateFamily(fetchKey))
			const silentUpgrade = detail === "full" && Boolean(existing.view) && !existing.stale
			if (!silentUpgrade) {
				markLoading({ key: fetchKey, loading: true, error: null })
			}
			try {
				const result = await getWorkspaceChanges(client, {
					sessionId: args.sessionId,
					scopes: [args.scope],
					cwd: args.directory || undefined,
					baseBranch: args.baseBranch ?? undefined,
					turnId: args.turnId ?? undefined,
					diffDetail: detail,
					maxDiffBytes: detail === "full" ? FULL_DIFF_LIMIT_BYTES : undefined,
					ignoreWhitespace: args.ignoreWhitespace || undefined,
				})
				const view = result.views.find((item) => item.scope === args.scope) as
					| WorkspaceChangeView
					| undefined
				if (!view) {
					setError({ key: fetchKey, error: "Workspace change view missing from response" })
					return
				}
				const current = appStore.get(workspaceChangesStateFamily(fetchKey))
				if (detail === "summary" && current.view?.unified_diff && !current.stale) {
					if (!silentUpgrade) markLoading({ key: fetchKey, loading: false })
					return
				}
				const next =
					detail === "summary"
						? mergeSummaryPreservingExpandState(current.view, view)
						: view
				setView({ key: fetchKey, view: next })
			} catch (error) {
				setError({
					key: fetchKey,
					error: error instanceof Error ? error.message : "Failed to load workspace changes",
				})
			} finally {
				inFlightRef.current.delete(flightKey)
			}
		},
		[isMockMode, markLoading, setError, setView],
	)

	const activeArgs = useMemo<FetchArgs>(
		() => ({
			sessionId,
			directory,
			scope,
			baseBranch: options.baseBranch,
			turnId,
			ignoreWhitespace,
		}),
		[directory, ignoreWhitespace, options.baseBranch, scope, sessionId, turnId],
	)

	const fetchChanges = useCallback(async () => {
		await fetchServer(activeArgs, "summary")
	}, [activeArgs, fetchServer])

	/**
	 * Expand a file: path-scoped Full + file sides via the server (remote-safe).
	 * Without `filePath`, fetches whole-tree Full patches; callers then re-enter
	 * per pending path so sides load path-scoped.
	 */
	const ensureFullPatches = useCallback(
		async (filePath?: string) => {
			if (!enabled || isMockMode) return
			const current = appStore.get(workspaceChangesStateFamily(key)).view
			if (!current) return
			const sidesFetched = sidesFetchedRef.current
			if (filePath) {
				if (fileExpandReady(current, filePath, sidesFetched, key)) return
			} else if (viewHasCompletePatches(current)) {
				// Whole-tree patches only; sides load via per-path ensureFullPatches.
				return
			}

			const client = getProjectClient(directory)
			if (!client) {
				setError({ key, error: "No workspace connection for this project" })
				return
			}
			const normalizedPath = filePath?.replace(/\\/g, "/")
			const flightKey = normalizedPath
				? `${key}\u001ffile\u001f${normalizedPath}`
				: `${key}\u001ffull`
			if (inFlightRef.current.has(flightKey)) return
			inFlightRef.current.add(flightKey)
			try {
				const result = await getWorkspaceChanges(client, {
					sessionId,
					scopes: [scope],
					cwd: directory || undefined,
					baseBranch: options.baseBranch ?? undefined,
					turnId: turnId ?? undefined,
					diffDetail: "full",
					maxDiffBytes: FULL_DIFF_LIMIT_BYTES,
					ignoreWhitespace: ignoreWhitespace || undefined,
					paths: normalizedPath ? [normalizedPath] : undefined,
					includeFileSides: Boolean(normalizedPath),
				})
				const patchView = result.views.find((item) => item.scope === scope) as
					| WorkspaceChangeView
					| undefined
				if (!patchView) {
					setError({ key, error: "Workspace change view missing from response" })
					return
				}
				if (normalizedPath) {
					sidesFetched.add(`${key}\u001f${normalizedPath}`)
				}
				const latest = appStore.get(workspaceChangesStateFamily(key)).view
				if (!latest) return
				setView({
					key,
					view: mergePathFullIntoView(latest, patchView),
				})
			} catch (error) {
				setError({
					key,
					error: error instanceof Error ? error.message : "Failed to load file diff",
				})
			} finally {
				inFlightRef.current.delete(flightKey)
			}
		},
		[
			directory,
			enabled,
			ignoreWhitespace,
			isMockMode,
			key,
			options.baseBranch,
			scope,
			sessionId,
			setError,
			setView,
			turnId,
		],
	)

	// Open path: Summary only. Never auto-Full.
	useEffect(() => {
		if (!enabled) return
		let cancelled = false
		void (async () => {
			if (needsSummary(key)) {
				await fetchServer(activeArgs, "summary")
				if (cancelled) return
			}
			if (prefetch && !isMockMode && directory) {
				for (const nextScope of PREFETCH_SCOPES) {
					if (cancelled) return
					if (nextScope === scope) continue
					const warmArgs: FetchArgs = {
						sessionId,
						directory,
						scope: nextScope,
						baseBranch: options.baseBranch,
						ignoreWhitespace,
					}
					const warmKey = cacheKeyFor(warmArgs)
					if (!needsSummary(warmKey)) continue
					void fetchServer(warmArgs, "summary")
				}
			}
		})()
		return () => {
			cancelled = true
		}
	}, [
		activeArgs,
		directory,
		enabled,
		fetchServer,
		ignoreWhitespace,
		isMockMode,
		key,
		options.baseBranch,
		prefetch,
		scope,
		sessionId,
	])

	const displayView = state.view ?? (state.loading ? fallbackState.view : null)
	const isInitialLoading = !displayView && state.loading
	const isRefreshing = Boolean(displayView) && state.loading
	const patchesPending = Boolean(displayView && displayView.files.length > 0 && !displayView.unified_diff)

	return {
		...state,
		view: displayView,
		key,
		latestTurnId,
		isInitialLoading,
		isRefreshing,
		patchesPending,
		refetch: fetchChanges,
		ensureFullPatches,
	}
}

/** Read cached Summary stats for every primary scope (dropdown preview). */
export function useWorkspaceScopeStatsPreview(
	sessionId: string,
	directory: string,
	options: {
		enabled?: boolean
		baseBranch?: string | null
		ignoreWhitespace?: boolean
	} = {},
): Partial<Record<WorkspaceChangeScope, ScopeStatsPreview>> {
	const latestTurnId = useAtomValue(latestWorkspaceTurnIdFamily(sessionId))
	const ignoreWhitespace = options.ignoreWhitespace ?? false
	const enabled = options.enabled ?? true

	const turnKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "turn",
				turnId: latestTurnId,
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, latestTurnId, options.baseBranch, sessionId],
	)
	const uncommittedKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "uncommitted",
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, options.baseBranch, sessionId],
	)
	const stagedKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "staged",
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, options.baseBranch, sessionId],
	)
	const unstagedKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "unstaged",
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, options.baseBranch, sessionId],
	)
	const branchKey = useMemo(
		() =>
			cacheKeyFor({
				sessionId,
				directory,
				scope: "branch",
				baseBranch: options.baseBranch,
				ignoreWhitespace,
			}),
		[directory, ignoreWhitespace, options.baseBranch, sessionId],
	)

	const turn = useAtomValue(workspaceChangesStateFamily(turnKey))
	const uncommitted = useAtomValue(workspaceChangesStateFamily(uncommittedKey))
	const staged = useAtomValue(workspaceChangesStateFamily(stagedKey))
	const unstaged = useAtomValue(workspaceChangesStateFamily(unstagedKey))
	const branch = useAtomValue(workspaceChangesStateFamily(branchKey))

	return useMemo(() => {
		if (!enabled) return {}
		const out: Partial<Record<WorkspaceChangeScope, ScopeStatsPreview>> = {}
		const take = (scope: WorkspaceChangeScope, view: WorkspaceChangeView | null | undefined) => {
			if (!view) return
			out[scope] = {
				additions: numberFromProtocol(view.stats?.additions) ?? 0,
				deletions: numberFromProtocol(view.stats?.deletions) ?? 0,
			}
		}
		take("turn", turn.view)
		take("uncommitted", uncommitted.view)
		take("staged", staged.view)
		take("unstaged", unstaged.view)
		take("branch", branch.view)
		return out
	}, [branch.view, enabled, staged.view, turn.view, uncommitted.view, unstaged.view])
}
