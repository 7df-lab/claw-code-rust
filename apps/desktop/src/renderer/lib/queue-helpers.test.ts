import { describe, expect, test } from "bun:test"
import { queueEntryText, queueRenderPreview } from "./queue-helpers"

describe("queue helpers", () => {
	test("queueRenderPreview collapses whitespace", () => {
		expect(queueRenderPreview("one\ntwo\n\nthree")).toBe("one two three")
	})

	test("queueEntryText prefers input text parts", () => {
		expect(
			queueEntryText({
				queueItemId: "q1",
				position: 0,
				preview: "preview only",
				input: [{ type: "text", text: "full text" }],
			}),
		).toBe("full text")
	})
})
