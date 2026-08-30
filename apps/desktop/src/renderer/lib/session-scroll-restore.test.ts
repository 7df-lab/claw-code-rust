import { describe, expect, test } from "bun:test"
import {
	EMPTY_SESSION_SCROLL_SNAPSHOT,
	planSessionScrollRestore,
} from "./session-scroll-restore"

describe("planSessionScrollRestore", () => {
	test("scrolls to bottom when no saved position", () => {
		expect(planSessionScrollRestore(EMPTY_SESSION_SCROLL_SNAPSHOT)).toEqual({
			action: "bottom",
		})
	})

	test("scrolls to bottom when user was at bottom", () => {
		expect(
			planSessionScrollRestore({ scrollTop: 420, atBottom: true, hasSnapshot: true }),
		).toEqual({
			action: "bottom",
		})
	})

	test("restores saved scroll position when not at bottom", () => {
		expect(
			planSessionScrollRestore({ scrollTop: 420, atBottom: false, hasSnapshot: true }),
		).toEqual({
			action: "restore",
			scrollTop: 420,
		})
	})

	test("restores top position when user was at scrollTop 0 with a snapshot", () => {
		expect(
			planSessionScrollRestore({ scrollTop: 0, atBottom: false, hasSnapshot: true }),
		).toEqual({
			action: "restore",
			scrollTop: 0,
		})
	})
})
