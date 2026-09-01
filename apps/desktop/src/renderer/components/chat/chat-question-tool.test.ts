import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import {
	isQuestionToolInput,
	parseQuestionToolEntries,
	questionToolSubtitle,
} from "./chat-question-tool"

const source = readFileSync(new URL("./chat-question-tool.tsx", import.meta.url), "utf8")

const questionInput = {
	questions: [
		{
			id: "environment",
			header: "Environment",
			question: "Where should this run?",
			isSecret: false,
		},
		{
			id: "style",
			header: "Style",
			question: "Which look do you prefer?",
			isSecret: false,
		},
	],
}

describe("question tool transcript", () => {
	test("treats generic tool parts with questions as question tools", () => {
		expect({
			byName: isQuestionToolInput("request_user_input"),
			byAlias: isQuestionToolInput("question"),
			byInput: isQuestionToolInput("tool", questionInput),
			generic: isQuestionToolInput("tool", { command: "ls" }),
		}).toEqual({
			byName: true,
			byAlias: true,
			byInput: true,
			generic: false,
		})
	})

	test("summarizes one or many questions without dumping JSON", () => {
		expect(
			questionToolSubtitle({
				tool: "tool",
				state: { status: "running", input: { questions: [questionInput.questions[0]] } },
			}),
		).toBe("Where should this run?")
		expect(
			questionToolSubtitle({
				tool: "request_user_input",
				state: { status: "completed", input: questionInput },
			}),
		).toBe("2 questions")
	})

	test("pairs answers from the tool result onto each question", () => {
		expect(
			parseQuestionToolEntries({
				tool: "request_user_input",
				state: {
					input: questionInput,
					output: JSON.stringify({
						answers: {
							environment: { answers: ["Local"] },
							style: { answers: ["Compact"] },
						},
					}),
				},
			}),
		).toEqual([
			{
				id: "environment",
				header: "Environment",
				question: "Where should this run?",
				isSecret: false,
				answer: "Local",
			},
			{
				id: "style",
				header: "Style",
				question: "Which look do you prefer?",
				isSecret: false,
				answer: "Compact",
			},
		])
	})

	test("keeps the expanded body quiet and 13px", () => {
		expect({
			thirteenPx: source.includes("text-[13px]"),
			waitingCopy: source.includes("Waiting for a reply"),
			noPrimaryTint: !source.includes("bg-primary") && !source.includes("text-primary"),
			checkIcon: source.includes("CheckIcon"),
		}).toEqual({
			thirteenPx: true,
			waitingCopy: true,
			noPrimaryTint: true,
			checkIcon: true,
		})
	})
})
