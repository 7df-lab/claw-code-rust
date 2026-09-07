import type {
	WorkspaceChangeCoverage,
	WorkspaceChangeScope,
	WorkspaceChangeSetStatus,
	WorkspaceChangeView,
	WorkspaceChangeViewStatus,
	WorkspaceChangesUpdatedEventProperties,
} from "@devo-ai/sdk/v2/client"
import { atom } from "jotai"
import { atomFamily } from "jotai/utils"

/** Scopes backed by live git state — partitioned by workspace cwd, not session. */
export const GIT_SCOPES: WorkspaceChangeScope[] = ["staged", "unstaged", "uncommitted", "branch"]

export type WorkspaceChangesCacheKeyInput = {
	/**
	 * Workspace root. Required partition for git scopes so sessions in the same
	 * project share one Changes cache.
	 */
	cwd?: string | null
	/** Session id — only partitions the turn scope (and legacy fallbacks). */
	sessionId?: string | null
	scope: WorkspaceChangeScope
	turnId?: string | null
	baseBranch?: string | null
	ignoreWhitespace?: boolean | null
}

export type WorkspaceChangesSummary = {
	sessionId: string
	turnId: string
	scope: WorkspaceChangeScope
	status: WorkspaceChangeViewStatus
	coverage: WorkspaceChangeCoverage
	changeSetStatus: WorkspaceChangeSetStatus
	stats: {
		files_changed: number
		additions: number
		deletions: number
	}
	version: number
	generatedAt: string
}

export type WorkspaceChangesState = {
	summary: WorkspaceChangesSummary | null
	view: WorkspaceChangeView | null
	loading: boolean
	stale: boolean
	error: string | null
}

function isGitScope(scope: WorkspaceChangeScope): boolean {
	return (GIT_SCOPES as string[]).includes(scope)
}

/**
 * Cache key for a Changes view.
 * - Git scopes: `cwd` + scope + baseBranch + whitespace (shared across sessions).
 * - Turn scope: `sessionId` + turnId (per-session artifact).
 */
export function workspaceChangesKey(input: WorkspaceChangesCacheKeyInput): string {
	if (input.scope === "turn" || !isGitScope(input.scope)) {
		return [
			input.sessionId ?? "",
			input.scope,
			input.turnId ?? "",
			"",
			input.ignoreWhitespace ? "w" : "",
		].join("\u001f")
	}
	return [
		input.cwd || input.sessionId || "",
		input.scope,
		"",
		input.baseBranch ?? "",
		input.ignoreWhitespace ? "w" : "",
	].join("\u001f")
}

function emptyWorkspaceChangesState(): WorkspaceChangesState {
	return {
		summary: null,
		view: null,
		loading: false,
		stale: true,
		error: null,
	}
}

function eventSummary(event: WorkspaceChangesUpdatedEventProperties): WorkspaceChangesSummary {
	return {
		sessionId: event.sessionID,
		turnId: event.turnID,
		scope: event.scope,
		status: event.status,
		coverage: event.coverage,
		changeSetStatus: event.changeSetStatus,
		stats: {
			files_changed: event.stats.filesChanged,
			additions: event.stats.additions,
			deletions: event.stats.deletions,
		},
		version: event.version,
		generatedAt: event.generatedAt,
	}
}

export const latestWorkspaceTurnIdFamily = atomFamily((_sessionId: string) =>
	atom<string | null>(null),
)

/** Maps a session to its workspace directory for git-scope invalidation. */
export const sessionWorkspaceDirectoryFamily = atomFamily((_sessionId: string) =>
	atom<string | null>(null),
)

export const registerSessionWorkspaceDirectoryAtom = atom(
	null,
	(_get, set, args: { sessionId: string; directory: string }) => {
		if (!args.sessionId || !args.directory) return
		set(sessionWorkspaceDirectoryFamily(args.sessionId), args.directory)
	},
)

export const workspaceChangesStateFamily = atomFamily((_key: string) =>
	atom<WorkspaceChangesState>(emptyWorkspaceChangesState()),
)

export const markWorkspaceChangesLoadingAtom = atom(
	null,
	(get, set, args: { key: string; loading: boolean; error?: string | null }) => {
		const current = get(workspaceChangesStateFamily(args.key))
		set(workspaceChangesStateFamily(args.key), {
			...current,
			loading: args.loading,
			error: args.error === undefined ? current.error : args.error,
		})
	},
)

/** Keep at most this many turn-scoped Changes views per session (LRU by write). */
const MAX_CACHED_TURN_VIEWS = 2

/** sessionId → most-recently-written turn cache keys (newest first). */
const turnViewKeysBySession = new Map<string, string[]>()

export const setWorkspaceChangesViewAtom = atom(
	null,
	(_get, set, args: { key: string; view: WorkspaceChangeView }) => {
		set(workspaceChangesStateFamily(args.key), {
			summary: null,
			view: args.view,
			loading: false,
			stale: false,
			error: null,
		})
		const parts = args.key.split("\u001f")
		if (parts[1] !== "turn") return
		const sessionId = parts[0]
		if (!sessionId) return
		const prev = turnViewKeysBySession.get(sessionId) ?? []
		const next = [args.key, ...prev.filter((item) => item !== args.key)].slice(
			0,
			MAX_CACHED_TURN_VIEWS,
		)
		// Drop heavy payloads from older turns. Do NOT atomFamily.remove() here —
		// removing atoms that still have React subscribers (or mid-write) triggers
		// "Should have a queue / Hooks conditionally".
		for (const stale of prev) {
			if (!next.includes(stale)) {
				set(workspaceChangesStateFamily(stale), emptyWorkspaceChangesState())
			}
		}
		turnViewKeysBySession.set(sessionId, next)
	},
)

export const setWorkspaceChangesErrorAtom = atom(
	null,
	(get, set, args: { key: string; error: string }) => {
		const current = get(workspaceChangesStateFamily(args.key))
		set(workspaceChangesStateFamily(args.key), {
			...current,
			loading: false,
			error: args.error,
		})
	},
)

export const applyWorkspaceChangesUpdatedAtom = atom(
	null,
	(get, set, event: WorkspaceChangesUpdatedEventProperties) => {
		const summary = eventSummary(event)
		if (event.scope === "turn") {
			set(latestWorkspaceTurnIdFamily(event.sessionID), event.turnID)
		}
		const cwd = get(sessionWorkspaceDirectoryFamily(event.sessionID))
		const key = workspaceChangesKey({
			cwd,
			sessionId: event.sessionID,
			scope: event.scope,
			turnId: event.scope === "turn" ? event.turnID : undefined,
		})
		const current = get(workspaceChangesStateFamily(key))
		set(workspaceChangesStateFamily(key), {
			...current,
			summary,
			stale: true,
			error: null,
		})
		if (event.scope === "turn") {
			// A finished turn changes the working tree, so every git-backed
			// scope cache for this workspace is potentially stale.
			const workspace = cwd || event.sessionID
			for (const scope of GIT_SCOPES) {
				for (const ignoreWhitespace of [false, true]) {
					const scopeKey = workspaceChangesKey({
						cwd: workspace,
						sessionId: event.sessionID,
						scope,
						ignoreWhitespace,
					})
					const scopeState = get(workspaceChangesStateFamily(scopeKey))
					if (scopeState.view || scopeState.summary) {
						set(workspaceChangesStateFamily(scopeKey), { ...scopeState, stale: true })
					}
				}
			}
		}
	},
)
