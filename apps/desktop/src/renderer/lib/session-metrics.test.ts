import { afterEach, describe, expect, test } from "bun:test"
import type { ChatTurn } from "../atoms/derived/session-chat"
import type { Message } from "./types"
import {
	computeLatestTurnTimerSplit,
	computeSessionMetrics,
	computeThoughtWorkTime,
	computeTurnWorkTime,
	computeTurnWorkTimeSplit,
	formatWorkDuration,
} from "./session-metrics"

const originalNow = Date.now

afterEach(() => {
	Date.now = originalNow
})

function turnWith(
	assistantMessages: ChatTurn["assistantMessages"],
	userCreated = 1_000,
	id = "u1",
): ChatTurn {
	return {
		id,
		userMessage: {
			info: { id, role: "user", time: { created: userCreated } },
			parts: [],
		},
		assistantMessages,
	} as ChatTurn
}

function formatTimerSplit(
	split: { completedMs: number; activeStartMs: number | null },
	now: number,
): string {
	return formatWorkDuration(split.completedMs + (split.activeStartMs != null ? now - split.activeStartMs : 0))
}

describe("turn duration metrics", () => {
	test("computes completed turn duration from user message to assistant completion", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 2_000, completed: 61_000 } },
				parts: [],
			},
		] as ChatTurn["assistantMessages"])

		expect(computeTurnWorkTime(turn)).toBe(60_000)
	})

	test("falls back to latest part timestamp when assistant completion is missing", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 2_000 } },
				parts: [
					{
						id: "tool-1",
						type: "tool",
						state: { status: "completed", time: { start: 3_000, end: 9_000 } },
					},
				],
			},
		] as ChatTurn["assistantMessages"])

		expect(computeTurnWorkTime(turn, { now: () => 99_000 })).toBe(8_000)
	})

	test("uses the latest persisted part end when assistant completion is recorded early", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 1_100, completed: 1_200 } },
				parts: [
					{
						id: "tool-1",
						type: "tool",
						state: { status: "completed", time: { start: 2_000, end: 31_000 } },
					},
				],
			},
		] as ChatTurn["assistantMessages"])

		expect(computeTurnWorkTime(turn)).toBe(30_000)
	})

	test("uses Date.now only for active turns", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 2_000 } },
				parts: [],
			},
		] as ChatTurn["assistantMessages"])

		expect({
			completed: computeTurnWorkTime(turn, { now: () => 99_000 }),
			active: computeTurnWorkTime(turn, { active: true, now: () => 11_000 }),
			split: computeTurnWorkTimeSplit(turn),
		}).toEqual({
			completed: 1_000,
			active: 10_000,
			split: { completedMs: 0, activeStartMs: 1_000 },
		})
	})

	test("drops implausible completed duration from incompatible historical timestamps", () => {
		const turn = turnWith([
			{
				info: { id: "a1", role: "assistant", time: { created: 2_000 } },
				parts: [
					{
						id: "tool-1",
						type: "tool",
						state: { status: "completed", time: { start: 3_000, end: 200_000_000 } },
					},
				],
			},
		] as ChatTurn["assistantMessages"])

		expect(computeTurnWorkTime(turn)).toBe(0)
	})
})

describe("thought duration metrics", () => {
	test("computes completed thought duration from part start to end", () => {
		expect(
			computeThoughtWorkTime({ time: { start: 1_000, end: 15_000 } }),
		).toBe(14_000)
	})

	test("returns 0 when completed thought is missing end", () => {
		expect(computeThoughtWorkTime({ time: { start: 1_000 } })).toBe(0)
	})

	test("uses Date.now only for active thoughts", () => {
		expect(
			computeThoughtWorkTime(
				{ time: { start: 1_000 } },
				{ active: true, now: () => 6_000 },
			),
		).toBe(5_000)
	})

	test("formats thought durations with the shared work duration formatter", () => {
		expect(formatWorkDuration(14_000)).toBe("14s")
		expect(formatWorkDuration(80_000)).toBe("1m 20s")
	})
})

describe("historical session timer guards", () => {
	test("does not treat historical ordering timestamps as active session timers", () => {
		Date.now = () => Date.parse("2026-06-25T12:00:00.000Z")
		const messages = [
			{ id: "history-0", role: "user", time: { created: 1 } },
			{
				id: "history-1",
				role: "assistant",
				parentID: "history-0",
				time: { created: 2 },
			},
		] as Message[]

		expect(computeSessionMetrics(messages)).toEqual({
			workTimeMs: 0,
			completedWorkTimeMs: 0,
			activeStartMs: null,
			cost: 0,
			tokens: {
				input: 0,
				output: 0,
				reasoning: 0,
				cacheRead: 0,
				cacheWrite: 0,
				total: 0,
			},
			exchangeCount: 1,
			userMessageCount: 1,
			assistantMessageCount: 1,
			modelDistribution: {},
			cacheEfficiency: 0,
			errorCount: 0,
			avgExchangeCost: 0,
			avgExchangeTimeMs: 0,
		})
	})
})

describe("top bar turn timer metrics", () => {
	test("returns zero when there are no turns yet", () => {
		const split = computeLatestTurnTimerSplit([], { mode: "stopped" })

		expect({ split, label: formatTimerSplit(split, 0) }).toEqual({
			split: { completedMs: 0, activeStartMs: null },
			label: "0s",
		})
	})

	test("starts a new optimistic user turn from the user message timestamp", () => {
		const start = Date.parse("2026-06-25T12:00:00.000Z")
		const now = start + 500
		const split = computeLatestTurnTimerSplit([turnWith([], start, "optimistic-1")], {
			mode: "running",
			now: () => now,
		})

		expect({ split, label: formatTimerSplit(split, now) }).toEqual({
			split: { completedMs: 0, activeStartMs: start },
			label: "1s",
		})
	})

	test("uses only the latest user turn so the timer resets on the next message", () => {
		const firstStart = Date.parse("2026-06-25T12:00:00.000Z")
		const nextStart = firstStart + 120_000
		const now = nextStart + 3_000
		const firstTurn = turnWith(
			[
				{
					info: {
						id: "a1",
						role: "assistant",
						time: { created: firstStart + 1_000, completed: firstStart + 60_000 },
					},
					parts: [],
				},
			] as ChatTurn["assistantMessages"],
			firstStart,
			"u1",
		)
		const nextTurn = turnWith([], nextStart, "optimistic-2")

		const split = computeLatestTurnTimerSplit([firstTurn, nextTurn], {
			mode: "running",
			now: () => now,
		})

		expect({ split, label: formatTimerSplit(split, now) }).toEqual({
			split: { completedMs: 0, activeStartMs: nextStart },
			label: "3s",
		})
	})

	test("stops on the completed turn timestamp when the session becomes idle", () => {
		const start = Date.parse("2026-06-25T12:00:00.000Z")
		const turn = turnWith(
			[
				{
					info: {
						id: "a1",
						role: "assistant",
						time: { created: start + 1_000, completed: start + 42_000 },
					},
					parts: [],
				},
			] as ChatTurn["assistantMessages"],
			start,
			"u1",
		)

		const split = computeLatestTurnTimerSplit([turn], { mode: "stopped" })

		expect({ split, label: formatTimerSplit(split, start + 99_000) }).toEqual({
			split: { completedMs: 42_000, activeStartMs: null },
			label: "42s",
		})
	})

	test("prefers the completed turn timestamp over a stale live fallback", () => {
		const start = Date.parse("2026-06-25T12:00:00.000Z")
		const turn = turnWith(
			[
				{
					info: {
						id: "a1",
						role: "assistant",
						time: { created: start + 1_000, completed: start + 42_000 },
					},
					parts: [],
				},
			] as ChatTurn["assistantMessages"],
			start,
			"u1",
		)

		const split = computeLatestTurnTimerSplit([turn], {
			mode: "stopped",
			fallbackCompletedMs: 99_000,
		})

		expect({ split, label: formatTimerSplit(split, start + 120_000) }).toEqual({
			split: { completedMs: 42_000, activeStartMs: null },
			label: "42s",
		})
	})

	test("keeps the last live elapsed value when an interrupted turn has no completion timestamp", () => {
		const start = Date.parse("2026-06-25T12:00:00.000Z")
		const turn = turnWith(
			[
				{
					info: { id: "a1", role: "assistant", time: { created: start + 1_000 } },
					parts: [],
				},
			] as ChatTurn["assistantMessages"],
			start,
			"u1",
		)

		const split = computeLatestTurnTimerSplit([turn], {
			mode: "stopped",
			fallbackCompletedMs: 17_000,
		})

		expect({ split, label: formatTimerSplit(split, start + 99_000) }).toEqual({
			split: { completedMs: 17_000, activeStartMs: null },
			label: "17s",
		})
	})

	test("does not start a huge live timer from historical ordering timestamps", () => {
		const now = Date.parse("2026-06-25T12:00:00.000Z")
		const split = computeLatestTurnTimerSplit([turnWith([], 1, "history-1")], {
			mode: "running",
			now: () => now,
		})

		expect({ split, label: formatTimerSplit(split, now) }).toEqual({
			split: { completedMs: 0, activeStartMs: null },
			label: "0s",
		})
	})
})
