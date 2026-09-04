import { describe, expect, test } from "bun:test"
import type { ReasoningPart, ToolPart } from "../../lib/types"
import {
	buildProcessTimeline,
	isReasoningPartActivelyStreaming,
	processTimelineRowId,
} from "./process-timeline"

function reasoning(id: string): { kind: "reasoning"; part: ReasoningPart } {
	return {
		kind: "reasoning",
		part: {
			id,
			type: "reasoning",
			text: `thought-${id}`,
			time: { start: 1, end: 2 },
		} as ReasoningPart,
	}
}

function tool(id: string, toolName = "read"): { kind: "tool"; part: ToolPart } {
	return {
		kind: "tool",
		part: {
			id,
			tool: toolName,
			type: "tool",
			state: { status: "completed", time: { start: 1, end: 2 }, output: "" },
		} as ToolPart,
	}
}

describe("buildProcessTimeline", () => {
	test("emits separate thought rows for each reasoning part", () => {
		expect(buildProcessTimeline([reasoning("r1"), tool("t1"), reasoning("r2"), tool("t2")])).toEqual([
			{ kind: "thought", part: reasoning("r1").part },
			{ kind: "tool", part: tool("t1").part },
			{ kind: "thought", part: reasoning("r2").part },
			{ kind: "tool", part: tool("t2").part },
		])
	})

	test("groups consecutive same-category tools without reasoning between them", () => {
		expect(buildProcessTimeline([reasoning("r1"), tool("t1"), tool("t2")])).toEqual([
			{ kind: "thought", part: reasoning("r1").part },
			{
				category: "explore",
				kind: "tool-group",
				tools: [tool("t1").part, tool("t2").part],
			},
		])
	})
})

function reasoningWithoutEnd(id: string): { kind: "reasoning"; part: ReasoningPart } {
	return {
		kind: "reasoning",
		part: {
			id,
			type: "reasoning",
			text: `thought-${id}`,
			time: { start: 1 },
		} as ReasoningPart,
	}
}

describe("isReasoningPartActivelyStreaming", () => {
	test("stops streaming once a tool follows the reasoning block", () => {
		const parts = [reasoningWithoutEnd("r1"), tool("t1"), reasoningWithoutEnd("r2")]

		expect({
			first: isReasoningPartActivelyStreaming(parts, reasoningWithoutEnd("r1").part),
			second: isReasoningPartActivelyStreaming(parts, reasoningWithoutEnd("r2").part),
		}).toEqual({
			first: false,
			second: true,
		})
	})

	test("stops streaming once assistant text follows the reasoning block", () => {
		const parts = [
			reasoningWithoutEnd("r1"),
			{ id: "txt", kind: "text" as const, text: "Here is the answer." },
		]

		expect(isReasoningPartActivelyStreaming(parts, reasoningWithoutEnd("r1").part)).toBe(false)
	})
})

describe("processTimelineRowId", () => {
	test("uses stable tool-group ids from category and tool ids", () => {
		const items = buildProcessTimeline([tool("t1"), tool("t2")])
		const group = items.find((item) => item.kind === "tool-group")
		expect(group).toBeTruthy()
		if (!group || group.kind !== "tool-group") return
		expect(processTimelineRowId(group, 0)).toBe(`group-${group.category}-t1+t2`)
		expect(processTimelineRowId(group, 99)).toBe(`group-${group.category}-t1+t2`)
	})
})
