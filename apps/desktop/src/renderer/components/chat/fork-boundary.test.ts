import { describe, expect, test } from "bun:test"
import type { ChatTurn } from "../../atoms/derived/session-chat"
import { forkBoundaryAfterTurnIndex } from "./fork-boundary"

function turn(turnId: string, created: number): ChatTurn {
	return {
		id: turnId,
		turnId,
		userMessage: {
			info: { id: `user-${turnId}`, role: "user", time: { created } },
			parts: [{ type: "text", text: "hello", sessionID: "s", messageID: `user-${turnId}`, id: "p1" }],
		},
		assistantMessages: [],
	}
}

describe("forkBoundaryAfterTurnIndex", () => {
	test("returns -1 when session is not a fork", () => {
		expect(forkBoundaryAfterTurnIndex([turn("t1", 100)], undefined, undefined, 500)).toBe(-1)
	})

	test("matches explicit fork turn id", () => {
		const turns = [turn("t1", 100), turn("t2", 200), turn("t3", 300)]
		expect(forkBoundaryAfterTurnIndex(turns, "parent", "t2", 500)).toBe(1)
	})

	test("uses fork creation time for tip forks", () => {
		const turns = [turn("t1", 100), turn("t2", 200), turn("t3", 600)]
		expect(forkBoundaryAfterTurnIndex(turns, "parent", undefined, 500)).toBe(1)
	})
})
