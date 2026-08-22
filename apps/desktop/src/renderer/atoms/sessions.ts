import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type { PermissionRequest, QuestionRequest, Session, SessionStatus } from "../lib/types"
import { messagesFamily } from "./messages"
import { partsFamily, partStorageKey } from "./parts"
import { viewedSessionIdAtom } from "./ui"

// ============================================================
// Constants
// ============================================================

/** Number of sessions to load per page when paginating */
export const SESSIONS_PAGE_SIZE = 5

// ============================================================
// Types
// ============================================================

/** Error type from session.error events */
export type SessionError = {
	name: string
	data: Record<string, unknown>
}

export type ProviderRetryStatus = {
	turnId: string
	attempt: number
	backoffMs: number
	provider: string
	model: string
	phase: "scheduled" | "resumed" | string
	message: string
}

/** Phases of worktree setup shown in the chat view's empty state */
export type SessionSetupPhase = "creating-worktree" | "starting-session" | null

/** Per-project pagination state for session loading */
export interface ProjectPaginationState {
	/** Whether the initial session fetch has been performed for this project */
	loaded: boolean
	/** Current limit used for session fetching */
	currentLimit: number
	/** Whether the last fetch returned fewer sessions than the limit (no more to load) */
	hasMore: boolean
	/** Whether a load-more request is in progress */
	loading: boolean
}

export interface SessionEntry {
	session: Session
	status: SessionStatus
	/** Pending permission requests */
	permissions: PermissionRequest[]
	/** Pending question requests */
	questions: QuestionRequest[]
	/** Project directory this session belongs to */
	directory: string
	/** Git branch at the time this session was created */
	branch?: string
	/** If set, the session runs in a git worktree at this path */
	worktreePath?: string
	/** The branch name auto-created for the worktree (e.g. "devo/fix-auth-bug") */
	worktreeBranch?: string
	/** Last session-level error (from session.error events) */
	error?: SessionError
	/** Worktree setup phase (shown in chat empty state while worktree is being created) */
	setupPhase?: SessionSetupPhase
	/** Renderer-local marker for a completed background turn waiting to be read. */
	hasUnreadCompletion?: boolean
	/** Active provider retry status for the current turn. */
	retryStatus?: ProviderRetryStatus
}

// ============================================================
// Index atom — tracks all known session IDs
// ============================================================

export const sessionIdsAtom = atom<Set<string>>(new Set<string>())

// ============================================================
// Per-session atom family
// ============================================================

export const sessionFamily = atomFamily((_sessionId: string) => atom<SessionEntry | null>(null))

// ============================================================
// Write-only action atoms
// ============================================================

export const upsertSessionAtom = atom(
	null,
	(
		get,
		set,
		args: {
			session: Session
			directory: string
		},
	) => {
		const { session, directory } = args
		const existing = get(sessionFamily(session.id))

		set(sessionFamily(session.id), {
			session,
			directory: existing?.directory ?? directory,
			status: existing?.status ?? { type: "idle" },
			permissions: existing?.permissions ?? [],
			questions: existing?.questions ?? [],
			branch: existing?.branch,
			worktreePath: existing?.worktreePath,
			worktreeBranch: existing?.worktreeBranch,
			error: existing?.error,
			setupPhase: existing?.setupPhase,
			hasUnreadCompletion: existing?.hasUnreadCompletion,
			retryStatus: existing?.retryStatus,
		})

		// Add to index
		const ids = get(sessionIdsAtom)
		if (!ids.has(session.id)) {
			const next = new Set(ids)
			next.add(session.id)
			set(sessionIdsAtom, next)
		}
	},
)

export const removeSessionAtom = atom(null, (get, set, sessionId: string) => {
	// Clean up message and part atoms to prevent memory leaks.
	// messagesFamily/partsFamily create atoms on demand and never remove them,
	// so we must explicitly clear and remove entries for deleted sessions.
	const messages = get(messagesFamily(sessionId))
	if (messages && messages.length > 0) {
		for (const msg of messages) {
			partsFamily.remove(partStorageKey(sessionId, msg.id))
			partsFamily.remove(msg.id)
		}
	}
	messagesFamily.remove(sessionId)

	sessionFamily.remove(sessionId)

	const ids = get(sessionIdsAtom)
	if (ids.has(sessionId)) {
		const next = new Set(ids)
		next.delete(sessionId)
		set(sessionIdsAtom, next)
	}
})

export const setSessionStatusAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			status: SessionStatus
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		const wasWorking = entry.status.type === "busy" || entry.status.type === "retry"
		const isWorking = args.status.type === "busy" || args.status.type === "retry"
		const completedTurn = wasWorking && args.status.type === "idle"
		const hasUnreadCompletion = isWorking
			? false
			: completedTurn
				? get(viewedSessionIdAtom) !== args.sessionId
				: entry.hasUnreadCompletion
		set(sessionFamily(args.sessionId), {
			...entry,
			status: args.status,
			hasUnreadCompletion,
			retryStatus: args.status.type === "idle" || args.status.type === "error" ? undefined : entry.retryStatus,
		})
	},
)


export const setProviderRetryStatusAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			status: ProviderRetryStatus | undefined
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		set(sessionFamily(args.sessionId), { ...entry, retryStatus: args.status })
	},
)

export const markSessionReadAtom = atom(null, (get, set, sessionId: string) => {
	const entry = get(sessionFamily(sessionId))
	if (!entry || !entry.hasUnreadCompletion) return
	set(sessionFamily(sessionId), { ...entry, hasUnreadCompletion: false })
})

export const setSessionErrorAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			error: SessionError | undefined
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		set(sessionFamily(args.sessionId), { ...entry, error: args.error })
	},
)

export const setSessionBranchAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			branch: string
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		set(sessionFamily(args.sessionId), { ...entry, branch: args.branch })
	},
)

export const setSessionWorktreeAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			worktreePath: string
			worktreeBranch: string
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		set(sessionFamily(args.sessionId), {
			...entry,
			worktreePath: args.worktreePath,
			worktreeBranch: args.worktreeBranch,
		})
	},
)

export const setSessionSetupPhaseAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			setupPhase: SessionSetupPhase
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		set(sessionFamily(args.sessionId), { ...entry, setupPhase: args.setupPhase })
	},
)

export const addPermissionAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			permission: PermissionRequest
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		if (entry.permissions.some((permission) => permission.id === args.permission.id)) return
		set(sessionFamily(args.sessionId), {
			...entry,
			permissions: [...entry.permissions, args.permission],
		})
	},
)

export const removePermissionAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			permissionId: string
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		set(sessionFamily(args.sessionId), {
			...entry,
			permissions: entry.permissions.filter((p) => p.id !== args.permissionId),
		})
	},
)

export const addQuestionAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			question: QuestionRequest
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		// Avoid duplicates
		if (entry.questions.some((q) => q.id === args.question.id)) return
		set(sessionFamily(args.sessionId), {
			...entry,
			questions: [...entry.questions, args.question],
		})
	},
)

export const removeQuestionAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessionId: string
			requestId: string
		},
	) => {
		const entry = get(sessionFamily(args.sessionId))
		if (!entry) return
		set(sessionFamily(args.sessionId), {
			...entry,
			questions: entry.questions.filter((q) => q.id !== args.requestId),
		})
	},
)

/**
 * Bulk-set sessions (used during project load).
 * Merges new sessions into the store without overwriting
 * permissions/questions that arrived via Native events before the fetch completed.
 *
 * Uses each session's own `directory` field from the API (falling back to
 * `args.directory`). This preserves sandbox (worktree) paths so the mapping
 * system in `agents.ts` can group them under the parent project.
 *
 * When `sandboxDirs` is provided, sessions whose directory matches a sandbox
 * get their `worktreePath` restored automatically, which makes the sidebar
 * show the worktree icon even after a window reload.
 */
export const setSessionsAtom = atom(
	null,
	(
		get,
		set,
		args: {
			sessions: Session[]
			statuses: Record<string, SessionStatus>
			directory: string
			/** Known sandbox directories for this project (from project.sandboxes) */
			sandboxDirs?: Set<string>
		},
	) => {
		const currentIds = get(sessionIdsAtom)
		const nextIds = new Set(currentIds)

		for (const session of args.sessions) {
			const existing = get(sessionFamily(session.id))
			const sessionDir = session.directory || args.directory
			const isSandbox = args.sandboxDirs?.has(sessionDir) ?? false

			set(sessionFamily(session.id), {
				session,
				status: args.statuses[session.id] ?? existing?.status ?? { type: "idle" },
				permissions: existing?.permissions ?? [],
				questions: existing?.questions ?? [],
				directory: existing?.directory ?? sessionDir,
				branch: existing?.branch,
				worktreePath: existing?.worktreePath ?? (isSandbox ? sessionDir : undefined),
				worktreeBranch: existing?.worktreeBranch,
				error: existing?.error,
				setupPhase: existing?.setupPhase,
				hasUnreadCompletion: existing?.hasUnreadCompletion,
			retryStatus: existing?.retryStatus,
			})
			nextIds.add(session.id)
		}

		if (nextIds.size !== currentIds.size) {
			set(sessionIdsAtom, nextIds)
		}
	},
)

// ============================================================
// Per-project session pagination
// ============================================================

/**
 * Tracks pagination state per project directory.
 * Used by the sidebar "Load more" button to know when to show/hide
 * and what limit to use for the next fetch.
 */
export const projectPaginationFamily = atomFamily((_directory: string) =>
	atom<ProjectPaginationState>({
		loaded: false,
		currentLimit: SESSIONS_PAGE_SIZE,
		hasMore: true,
		loading: false,
	}),
)

/**
 * Write-only atom to update pagination state after a session load.
 * Called by loadMoreProjectSessions() after fetching the next page.
 */
export const updateProjectPaginationAtom = atom(
	null,
	(
		_get,
		set,
		args: {
			directory: string
			fetchedCount: number
			limit: number
			hasMore?: boolean
		},
	) => {
		set(projectPaginationFamily(args.directory), {
			loaded: true,
			currentLimit: args.limit,
			hasMore: args.hasMore ?? args.fetchedCount >= args.limit,
			loading: false,
		})
	},
)

/**
 * Write-only atom to mark a project's session load as in progress.
 */
export const setProjectPaginationLoadingAtom = atom(null, (get, set, directory: string) => {
	const current = get(projectPaginationFamily(directory))
	set(projectPaginationFamily(directory), { ...current, loading: true })
})

/**
 * Write-only atom to reset pagination state for a list of directories.
 * Called on server switch so expanded projects re-fetch sessions from the new server.
 */
export const resetProjectPaginationAtom = atom(null, (_get, set, directories: string[]) => {
	const initial: ProjectPaginationState = {
		loaded: false,
		currentLimit: SESSIONS_PAGE_SIZE,
		hasMore: true,
		loading: false,
	}
	for (const dir of directories) {
		set(projectPaginationFamily(dir), initial)
	}
})
