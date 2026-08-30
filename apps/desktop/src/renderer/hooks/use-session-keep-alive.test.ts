import { describe, expect, test } from "bun:test"
import {
	SESSION_KEEP_ALIVE_CAPACITY,
	evictMountedSession,
	initialMountedSessionIds,
	updateMountedSessionIds,
} from "./use-session-keep-alive-logic"

describe("session keep-alive logic", () => {
	test("initialMountedSessionIds seeds the active session", () => {
		expect(initialMountedSessionIds("session-a")).toEqual(["session-a"])
		expect(initialMountedSessionIds(null)).toEqual([])
	})

	test("updateMountedSessionIds promotes the active session", () => {
		expect(updateMountedSessionIds(["session-a"], "session-b")).toEqual([
			"session-b",
			"session-a",
		])
	})

	test("updateMountedSessionIds evicts the oldest session at capacity", () => {
		const next = updateMountedSessionIds(
			["session-d", "session-c", "session-b"],
			"session-e",
			SESSION_KEEP_ALIVE_CAPACITY,
		)
		expect(next).toEqual(["session-e", "session-d", "session-c"])
	})

	test("evictMountedSession removes a mounted session", () => {
		expect(evictMountedSession(["session-b", "session-a"], "session-a")).toEqual(["session-b"])
	})
})
