import { atom } from "jotai"
import { atomFamily, atomWithStorage } from "jotai/utils"
import type { FileDiff, WorkspaceChangeScope } from "../lib/types"
import { REVIEW_PANEL_DEFAULT_WIDTH_PX } from "../lib/review-panel-width"

export const commandPaletteOpenAtom = atom(false)

/**
 * The session ID currently being viewed in the main content area.
 * Set by the router/session view when the user navigates to a session.
 * Used by metrics atoms to skip expensive recomputation for background sessions.
 */
export const viewedSessionIdAtom = atom<string | null>(null)

/**
 * Last non-settings route visited before opening Settings.
 * Used to restore navigation when leaving Settings.
 */
export const lastAppRouteAtom = atom<string | null>(null)

/** Session kept mounted in the background while Settings is open. */
export interface SettingsBackgroundSession {
	sessionId: string
	projectSlug: string
}

export const settingsBackgroundSessionAtom = atom<SettingsBackgroundSession | null>(null)

/** Whether the Settings overlay is covering the main content (set by SidebarLayout). */
export const settingsOverlayOpenAtom = atom(false)

/** Whether Customize is showing in the main content pane (does not change the route). */
export const customizeOpenAtom = atom(false)

export interface SessionScrollSnapshot {
	scrollTop: number
	atBottom: boolean
	/** Distinguishes an unvisited session from a deliberate scrollTop of 0. */
	hasSnapshot: boolean
}

const EMPTY_SCROLL_SNAPSHOT: SessionScrollSnapshot = {
	scrollTop: 0,
	atBottom: true,
	hasSnapshot: false,
}

/** Last known scroll snapshot for a session's chat view. */
export const sessionScrollSnapshotFamily = atomFamily((_sessionId: string) =>
	atom<SessionScrollSnapshot>(EMPTY_SCROLL_SNAPSHOT),
)

/** @deprecated Use sessionScrollSnapshotFamily */
export const sessionScrollTopFamily = atomFamily((sessionId: string) =>
	atom(
		(get) => {
			const snapshot = get(sessionScrollSnapshotFamily(sessionId))
			return snapshot.hasSnapshot ? snapshot.scrollTop : null
		},
		(get, set, scrollTop: number | null) => {
			if (scrollTop == null) {
				set(sessionScrollSnapshotFamily(sessionId), EMPTY_SCROLL_SNAPSHOT)
				return
			}
			const current = get(sessionScrollSnapshotFamily(sessionId))
			set(sessionScrollSnapshotFamily(sessionId), {
				...current,
				scrollTop,
				hasSnapshot: true,
			})
		},
	),
)

/** @deprecated Use sessionScrollSnapshotFamily */
export const sessionAtBottomFamily = atomFamily((sessionId: string) =>
	atom(
		(get) => get(sessionScrollSnapshotFamily(sessionId)).atBottom,
		(get, set, atBottom: boolean) => {
			const current = get(sessionScrollSnapshotFamily(sessionId))
			set(sessionScrollSnapshotFamily(sessionId), { ...current, atBottom })
		},
	),
)

// ============================================================
// Review Panel State
// ============================================================

/** Whether the review panel is open (resets to closed on app start) */
export const reviewPanelOpenAtom = atom(false)

/** Kind of view hosted in a right-panel browser tab. */
export type RightPanelTabKind = "changes" | "terminal"

export type RightPanelTab = {
	id: string
	kind: RightPanelTabKind
	title: string
}

export type RightPanelTabsState = {
	tabs: RightPanelTab[]
	activeId: string
}

const DEFAULT_CHANGES_TAB: RightPanelTab = {
	id: "changes-main",
	kind: "changes",
	title: "Changes",
}

export const DEFAULT_RIGHT_PANEL_CHANGES_TAB = DEFAULT_CHANGES_TAB

export const rightPanelTabsAtom = atom<RightPanelTabsState>({
	tabs: [DEFAULT_CHANGES_TAB],
	activeId: DEFAULT_CHANGES_TAB.id,
})

/** @deprecated Prefer rightPanelTabsAtom; kept for any leftover single-tab reads. */
export type RightPanelTabId = RightPanelTabKind
export const rightPanelTabAtom = atom(
	(get) => {
		const state = get(rightPanelTabsAtom)
		return state.tabs.find((tab) => tab.id === state.activeId)?.kind ?? "changes"
	},
	(_get, set, kind: RightPanelTabKind) => {
		set(rightPanelTabsAtom, (prev) => {
			const existing = prev.tabs.find((tab) => tab.kind === kind)
			if (existing) return { ...prev, activeId: existing.id }
			const next: RightPanelTab = {
				id: `${kind}-${Date.now().toString(36)}`,
				kind,
				title: kind === "changes" ? "Changes" : kind === "terminal" ? "Terminal" : kind,
			}
			return { tabs: [...prev.tabs, next], activeId: next.id }
		})
	},
)

/** Session-header context occupancy overlay (aligned to the conversation column). */
export const contextUsageOpenAtom = atom(false)

/**
 * File path to highlight in the review panel.
 * Set by external components (e.g. edit tool card "View diff" button).
 * The ReviewPanel subscribes to this and syncs it with its local selectedFile state.
 * Cleared automatically after the panel consumes it.
 */
export const reviewPanelSelectedFileAtom = atom<string | null>(null)

/**
 * Action atom: opens the review panel and jumps to a specific file.
 * Usage: `const viewDiff = useSetAtom(viewFileInDiffPanelAtom)`
 *        `viewDiff("src/foo.ts")`
 */
export const viewFileInDiffPanelAtom = atom(null, (_get, set, filePath: string) => {
	set(reviewPanelOpenAtom, true)
	set(reviewPanelSelectedFileAtom, filePath)
})

/** Diff display style preference */
export type DiffStyle = "unified" | "split"

/** Review panel user preferences (persisted to localStorage) */
export interface ReviewPanelSettings {
	/** Diff rendering style: unified (single column) or split (side-by-side) */
	diffStyle: DiffStyle
	/** Whether the review panel is expanded to full width */
	expanded: boolean
	/** User-resized panel width in pixels (ignored while expanded) */
	widthPx: number
	/** Server-side `--ignore-all-space` for git-backed scopes */
	ignoreWhitespace: boolean
	/** Wrap long diff lines instead of horizontal scrolling */
	wordWrap: boolean
}

export const reviewPanelSettingsAtom = atomWithStorage<ReviewPanelSettings>(
	"devo:review-panel-settings",
	{
		diffStyle: "unified",
		expanded: false,
		widthPx: REVIEW_PANEL_DEFAULT_WIDTH_PX,
		ignoreWhitespace: false,
		wordWrap: false,
	},
)

/**
 * Per-workspace Changes UI (scope, expand toggles, find). Shared across
 * sessions in the same project directory; survives panel hide / tab remounts.
 */
export type ReviewPanelWorkspaceUi = {
	scope: WorkspaceChangeScope
	baseBranch: string | null
	/** Expanded/collapsed overrides keyed by scope, then file path. */
	userTogglesByScope: Partial<Record<WorkspaceChangeScope, Record<string, boolean>>>
	filterOpen: boolean
	filterQuery: string
}

const DEFAULT_REVIEW_PANEL_WORKSPACE_UI: ReviewPanelWorkspaceUi = {
	// Default to working-tree scope; lists load via Electron local git (IPC).
	// "turn" still uses the Devo server (checkpoint baseline).
	scope: "uncommitted",
	baseBranch: null,
	userTogglesByScope: {},
	filterOpen: false,
	filterQuery: "",
}

export const reviewPanelWorkspaceUiFamily = atomFamily((_directory: string) =>
	atom<ReviewPanelWorkspaceUi>(DEFAULT_REVIEW_PANEL_WORKSPACE_UI),
)

/** @deprecated Use reviewPanelWorkspaceUiFamily — kept for any leftover imports. */
export type ReviewPanelSessionUi = ReviewPanelWorkspaceUi
/** @deprecated Use reviewPanelWorkspaceUiFamily */
export const reviewPanelSessionUiFamily = reviewPanelWorkspaceUiFamily

/** Per-session diff data from the Devo API */
export const sessionDiffFamily = atomFamily((_sessionId: string) => atom<FileDiff[]>([]))

/** Write-only atom to update session diff data */
export const setSessionDiffAtom = atom(
	null,
	(_get, set, args: { sessionId: string; diffs: FileDiff[] }) => {
		set(sessionDiffFamily(args.sessionId), args.diffs)
	},
)

/** Per-session diff filter: null = all session changes, string = specific messageID */
export const diffFilterFamily = atomFamily((_sessionId: string) => atom<string | null>(null))

/** Computed total stats for a session's diffs (all files, including generated) */
export const sessionDiffStatsFamily = atomFamily((sessionId: string) =>
	atom((get) => {
		const diffs = get(sessionDiffFamily(sessionId))
		let additions = 0
		let deletions = 0
		for (const diff of diffs) {
			additions += diff.additions
			deletions += diff.deletions
		}
		return { additions, deletions, fileCount: diffs.length }
	}),
)
