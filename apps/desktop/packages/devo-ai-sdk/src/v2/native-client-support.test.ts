import { describe, expect, test } from "bun:test"
import { toolPartFromUpdate } from "./native-client-support"

describe("native tool name mapping", () => {
	test("maps request_user_input kind and questions input to a question tool", () => {
		const fromKind = toolPartFromUpdate(
			"session-1",
			{
				kind: "request_user_input",
				rawInput: {
					questions: [
						{
							id: "environment",
							header: "Environment",
							question: "Where should this run?",
						},
					],
				},
			},
			undefined,
			1,
		)
		const fromInput = toolPartFromUpdate(
			"session-1",
			{
				rawInput: {
					questions: [{ id: "environment", question: "Where should this run?" }],
				},
			},
			undefined,
			1,
		)
		expect({
			fromKind: fromKind.tool,
			fromInput: fromInput.tool,
		}).toEqual({
			fromKind: "request_user_input",
			fromInput: "request_user_input",
		})
	})
})
