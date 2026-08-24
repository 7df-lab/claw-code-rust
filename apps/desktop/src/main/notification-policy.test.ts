import { describe, expect, test } from "bun:test"
import {
	applyWatcherEvent,
	isAppInForeground,
	isWorkingSessionStatus,
	shouldAnnounceCompletion,
	shouldShowLiveNotification,
	type ForegroundWindowLike,
	type SessionState,
} from "./notification-policy"

function windowLike(overrides: Partial<ForegroundWindowLike> = {}): ForegroundWindowLike {
	return {
		isDestroyed: () => false,
		isVisible: () => true,
		isFocused: () => true,
		isMinimized: () => false,
		...overrides,
	}
}

describe("notification policy", () => {
	test("treats busy and retry as working statuses", () => {
		expect(isWorkingSessionStatus("busy")).toBe(true)
		expect(isWorkingSessionStatus("retry")).toBe(true)
		expect(isWorkingSessionStatus("idle")).toBe(false)
		expect(isWorkingSessionStatus(undefined)).toBe(false)
	})

	test("announces completion only for a live working-to-idle transition", () => {
		expect(
			shouldAnnounceCompletion({
				hydrating: false,
				isSubAgent: false,
				previousStatus: "busy",
				nextStatus: "idle",
			}),
		).toBe(true)
		expect(
			shouldAnnounceCompletion({
				hydrating: false,
				isSubAgent: false,
				previousStatus: "retry",
				nextStatus: "idle",
			}),
		).toBe(true)
		expect(
			shouldAnnounceCompletion({
				hydrating: true,
				isSubAgent: false,
				previousStatus: "busy",
				nextStatus: "idle",
			}),
		).toBe(false)
		expect(
			shouldAnnounceCompletion({
				hydrating: false,
				isSubAgent: true,
				previousStatus: "busy",
				nextStatus: "idle",
			}),
		).toBe(false)
		expect(
			shouldAnnounceCompletion({
				hydrating: false,
				isSubAgent: false,
				previousStatus: "idle",
				nextStatus: "idle",
			}),
		).toBe(false)
		expect(
			shouldAnnounceCompletion({
				hydrating: false,
				isSubAgent: false,
				previousStatus: undefined,
				nextStatus: "idle",
			}),
		).toBe(false)
	})

	test("suppresses live toasts while hydrating historical replay", () => {
		expect(shouldShowLiveNotification(true)).toBe(false)
		expect(shouldShowLiveNotification(false)).toBe(true)
	})

	test("treats a visible focused window as foreground", () => {
		expect(isAppInForeground([windowLike()])).toBe(true)
		expect(isAppInForeground([windowLike({ isFocused: () => false })])).toBe(false)
		expect(isAppInForeground([windowLike({ isMinimized: () => true })])).toBe(false)
		expect(isAppInForeground([windowLike({ isVisible: () => false })])).toBe(false)
		expect(isAppInForeground([])).toBe(false)
	})
})

describe("applyWatcherEvent", () => {
	function session(status: string, parentID?: string): SessionState {
		return { status, title: "Fix the tray menu", directory: "/repo", parentID }
	}

	test("does not toast completion while hydrating a busy-to-idle replay", () => {
		const sessions = new Map<string, SessionState>([["s1", session("busy")]])
		const result = applyWatcherEvent(
			{
				directory: "/repo",
				payload: {
					type: "session.status",
					properties: { sessionID: "s1", status: { type: "idle" } },
				},
			},
			{ sessions, pendingCount: 0, hydrating: true },
		)

		expect(result.notifications).toEqual([])
		expect(sessions.get("s1")?.status).toBe("idle")
	})

	test("toasts completion after hydration when a live session finishes", () => {
		const sessions = new Map<string, SessionState>([["s1", session("busy")]])
		const result = applyWatcherEvent(
			{
				directory: "/repo",
				payload: {
					type: "session.status",
					properties: { sessionID: "s1", status: { type: "idle" } },
				},
			},
			{ sessions, pendingCount: 0, hydrating: false },
		)

		expect(result.notifications).toEqual([
			{
				type: "completed",
				sessionId: "s1",
				title: "Agent finished",
				body: "Fix the tray menu",
				directory: "/repo",
			},
		])
	})

	test("does not toast replayed permission requests while hydrating", () => {
		const sessions = new Map<string, SessionState>([["s1", session("busy")]])
		const result = applyWatcherEvent(
			{
				directory: "/repo",
				payload: {
					type: "permission.asked",
					properties: { sessionID: "s1", id: "p1", permission: "Run npm test" },
				},
			},
			{ sessions, pendingCount: 0, hydrating: true },
		)

		expect(result.notifications).toEqual([])
		expect(result.pendingCount).toBe(1)
	})
})
