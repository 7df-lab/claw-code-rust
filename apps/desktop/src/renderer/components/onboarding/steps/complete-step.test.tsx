import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { CompleteStep } from "./complete-step"

describe("CompleteStep", () => {
	test("does not offer Claude Code or OpenCode import", () => {
		const markup = renderToStaticMarkup(<CompleteStep devoVersion="0.1.36" onFinish={() => {}} />)

		expect({
			hasReadyTitle: markup.includes("all set"),
			hasStartCta: markup.includes("Start Building"),
			mentionsClaudeCode: markup.toLowerCase().includes("claude code"),
			mentionsOpenCode: markup.toLowerCase().includes("opencode"),
			mentionsMigrate: markup.toLowerCase().includes("migrate"),
		}).toEqual({
			hasReadyTitle: true,
			hasStartCta: true,
			mentionsClaudeCode: false,
			mentionsOpenCode: false,
			mentionsMigrate: false,
		})
	})
})
