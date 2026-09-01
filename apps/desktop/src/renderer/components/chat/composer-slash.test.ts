import { describe, expect, test } from "bun:test"
import { parseComposerSlash, goalPromptText } from "./composer-slash"

describe("composer slash parsing", () => {
	test("recognizes first-party commands and aliases", () => {
		expect(parseComposerSlash("/plan")).toEqual({ name: "plan", args: "" })
		expect(parseComposerSlash("/goal write tests")).toEqual({
			name: "goal",
			args: "write tests",
		})
		expect(parseComposerSlash("/btw what is this")).toEqual({
			name: "side",
			args: "what is this",
		})
		expect(parseComposerSlash("/compact")).toEqual({ name: "compact", args: "" })
		expect(parseComposerSlash("hello")).toBeNull()
		expect(parseComposerSlash("/unknown")).toBeNull()
	})

	test("prefixes goal chip submissions", () => {
		expect(goalPromptText("ship the api")).toBe("/goal ship the api")
	})
})
