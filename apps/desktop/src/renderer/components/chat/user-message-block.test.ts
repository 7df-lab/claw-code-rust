import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./user-message-block.tsx", import.meta.url), "utf8")

describe("UserMessageBlock", () => {
	test("keeps copy and edit actions close and hover-only", () => {
		expect({
			rightAlignedInlineBubble:
				source.includes('relative inline-block max-w-[min(36rem,85%)]') &&
				source.includes("text-left align-top"),
			noFlexShrinkWrap: !source.includes("w-max") && !source.includes("min-w-0"),
			overlaysJustBelowBubble: source.includes("absolute top-full right-0") && source.includes("pt-0.5"),
			hoverOnlyOnPointerDevices: source.includes("[@media(hover:hover)]:opacity-0") &&
				source.includes("group-hover:opacity-100"),
			focusKeepsActionsVisible: source.includes("group-focus-within:opacity-100"),
		}).toEqual({
			rightAlignedInlineBubble: true,
			noFlexShrinkWrap: true,
			overlaysJustBelowBubble: true,
			hoverOnlyOnPointerDevices: true,
			focusKeepsActionsVisible: true,
		})
	})
})
