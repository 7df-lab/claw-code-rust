import { describe, expect, test } from "bun:test"
import type {
	WorkspaceChangeScope,
	WorkspaceChangeView,
	WorkspaceChangesUpdatedEventProperties,
} from "@devo-ai/sdk/v2/client"
import { createStore } from "jotai"
import {
	applyWorkspaceChangesUpdatedAtom,
	GIT_SCOPES,
	latestWorkspaceTurnIdFamily,
	registerSessionWorkspaceDirectoryAtom,
	setWorkspaceChangesViewAtom,
	workspaceChangesKey,
	workspaceChangesStateFamily,
} from "./workspace-changes"
import type { WorkspaceChangesSummary } from "./workspace-changes"

const SESSION = "ses_test"
const SESSION_B = "ses_other"
const TURN = "turn_1"
const CWD = "C:/tmp/repo"

function turnEvent(scope: WorkspaceChangeScope): WorkspaceChangesUpdatedEventProperties {
	return {
		sessionID: SESSION,
		turnID: TURN,
		scope,
		status: "ready",
		coverage: "git_visible",
		changeSetStatus: scope === "turn" ? "finalized" : "accumulating",
		stats: { filesChanged: 2, additions: 10, deletions: 4 },
		version: 1,
		generatedAt: "2026-09-07T00:00:00Z",
	}
}

function fakeView(scope: WorkspaceChangeScope): WorkspaceChangeView {
	return {
		scope,
		status: "ready",
		workspace_root: "C:/tmp/repo",
		coverage: "git_visible",
		attribution: "git_working_tree",
		change_set_status: "accumulating",
		files: [],
		stats: { files_changed: 0n, additions: 0n, deletions: 0n },
		warnings: [],
		generated_at: "2026-09-07T00:00:00Z",
	}
}

function seedView(
	store: ReturnType<typeof createStore>,
	scope: WorkspaceChangeScope,
	ignoreWhitespace: boolean,
) {
	store.set(setWorkspaceChangesViewAtom, {
		key: workspaceChangesKey({ cwd: CWD, sessionId: SESSION, scope, ignoreWhitespace }),
		view: fakeView(scope),
	})
}

function summaryOf(scope: WorkspaceChangeScope): WorkspaceChangesSummary {
	return {
		sessionId: SESSION,
		turnId: TURN,
		scope,
		status: "ready",
		coverage: "git_visible",
		changeSetStatus: "accumulating",
		stats: { files_changed: 1, additions: 1, deletions: 0 },
		version: 1,
		generatedAt: "2026-09-07T00:00:00Z",
	}
}

describe("workspaceChangesKey", () => {
	test("distinguishes whitespace variants", () => {
		const base = { cwd: CWD, scope: "uncommitted" as const }
		expect(workspaceChangesKey({ ...base, ignoreWhitespace: false })).not.toBe(
			workspaceChangesKey({ ...base, ignoreWhitespace: true }),
		)
		expect(workspaceChangesKey(base)).toBe(workspaceChangesKey({ ...base, ignoreWhitespace: null }))
		expect(workspaceChangesKey(base)).toBe(workspaceChangesKey({ ...base, ignoreWhitespace: false }))
	})

	test("git scopes share a cache across sessions in the same workspace", () => {
		const a = workspaceChangesKey({
			cwd: CWD,
			sessionId: SESSION,
			scope: "unstaged",
		})
		const b = workspaceChangesKey({
			cwd: CWD,
			sessionId: SESSION_B,
			scope: "unstaged",
		})
		expect(a).toBe(b)
	})

	test("turn scope stays partitioned by session", () => {
		const a = workspaceChangesKey({
			cwd: CWD,
			sessionId: SESSION,
			scope: "turn",
			turnId: TURN,
		})
		const b = workspaceChangesKey({
			cwd: CWD,
			sessionId: SESSION_B,
			scope: "turn",
			turnId: TURN,
		})
		expect(a).not.toBe(b)
	})
})

describe("applyWorkspaceChangesUpdatedAtom", () => {
	test("records the latest turn id and stales the turn key", () => {
		const store = createStore()
		store.set(applyWorkspaceChangesUpdatedAtom, turnEvent("turn"))

		expect(store.get(latestWorkspaceTurnIdFamily(SESSION))).toBe(TURN)

		const state = store.get(
			workspaceChangesStateFamily(
				workspaceChangesKey({ sessionId: SESSION, scope: "turn", turnId: TURN }),
			),
		)
		expect(state.summary?.stats.files_changed).toBe(2)
		expect(state.stale).toBe(true)
		expect(state.error).toBeNull()
	})

	test("turn events stale every git scope in both whitespace variants without dropping views", () => {
		const store = createStore()
		store.set(registerSessionWorkspaceDirectoryAtom, { sessionId: SESSION, directory: CWD })

		for (const scope of GIT_SCOPES) {
			for (const ignoreWhitespace of [false, true]) seedView(store, scope, ignoreWhitespace)
		}
		const unstagedWhitespaceKey = workspaceChangesKey({
			cwd: CWD,
			scope: "unstaged",
			ignoreWhitespace: true,
		})
		store.set(workspaceChangesStateFamily(unstagedWhitespaceKey), {
			summary: summaryOf("unstaged"),
			view: fakeView("unstaged"),
			loading: false,
			stale: false,
			error: null,
		})
		const stagedWhitespaceKey = workspaceChangesKey({
			cwd: CWD,
			scope: "staged",
			ignoreWhitespace: true,
		})
		store.set(workspaceChangesStateFamily(stagedWhitespaceKey), {
			summary: summaryOf("staged"),
			view: fakeView("staged"),
			loading: false,
			stale: false,
			error: null,
		})

		store.set(applyWorkspaceChangesUpdatedAtom, turnEvent("turn"))

		for (const scope of GIT_SCOPES) {
			for (const ignoreWhitespace of [false, true]) {
				const state = store.get(
					workspaceChangesStateFamily(
						workspaceChangesKey({ cwd: CWD, scope, ignoreWhitespace }),
					),
				)
				expect(state.stale).toBe(true)
				expect(state.view).not.toBeNull()
			}
		}
		expect(store.get(workspaceChangesStateFamily(stagedWhitespaceKey)).summary).toEqual(
			summaryOf("staged"),
		)
	})

	test("non-turn events only stale their own key", () => {
		const store = createStore()
		store.set(registerSessionWorkspaceDirectoryAtom, { sessionId: SESSION, directory: CWD })
		store.set(applyWorkspaceChangesUpdatedAtom, turnEvent("uncommitted"))

		expect(store.get(latestWorkspaceTurnIdFamily(SESSION))).toBeNull()
		expect(
			store.get(
				workspaceChangesStateFamily(
					workspaceChangesKey({ cwd: CWD, scope: "uncommitted" }),
				),
			).stale,
		).toBe(true)
		expect(
			store.get(
				workspaceChangesStateFamily(
					workspaceChangesKey({ cwd: CWD, scope: "uncommitted", ignoreWhitespace: true }),
				),
			).summary,
		).toBeNull()
	})

	test("keeps only the two newest turn views per session", () => {
		const store = createStore()
		const key1 = workspaceChangesKey({ sessionId: SESSION, scope: "turn", turnId: "t1" })
		const key2 = workspaceChangesKey({ sessionId: SESSION, scope: "turn", turnId: "t2" })
		const key3 = workspaceChangesKey({ sessionId: SESSION, scope: "turn", turnId: "t3" })
		store.set(setWorkspaceChangesViewAtom, { key: key1, view: fakeView("turn") })
		store.set(setWorkspaceChangesViewAtom, { key: key2, view: fakeView("turn") })
		store.set(setWorkspaceChangesViewAtom, { key: key3, view: fakeView("turn") })

		// Oldest turn payload is cleared (not atomFamily.remove — that breaks subscribers).
		expect(store.get(workspaceChangesStateFamily(key1)).view).toBeNull()
		expect(store.get(workspaceChangesStateFamily(key2)).view).not.toBeNull()
		expect(store.get(workspaceChangesStateFamily(key3)).view).not.toBeNull()
	})
})
