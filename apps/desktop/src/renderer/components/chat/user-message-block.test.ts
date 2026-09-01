import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./user-message-block.tsx", import.meta.url), "utf8")

describe("UserMessageBlock", () => {
	test("keeps copy and edit actions close and hover-only", () => {
		expect({
			rightAlignedInlineBubble:
				source.includes("inline-flex max-w-[min(36rem,85%)]") &&
				source.includes("items-end gap-0 text-left"),
			noFlexShrinkWrap: !source.includes("w-max") && !source.includes("min-w-0"),
			zeroHeightActionAnchor:
				source.includes("relative h-0 w-full") && source.includes("absolute top-0 right-0"),
			hoverBridgeCoversActionZone:
				source.includes("absolute inset-x-0 -top-1 z-0 h-8"),
			hoverOnlyOnPointerDevices: source.includes("[@media(hover:hover)]:opacity-0") &&
				source.includes("group-hover/user-msg:opacity-100"),
			focusKeepsActionsVisible: source.includes("group-focus-within/user-msg:opacity-100"),
		}).toEqual({
			rightAlignedInlineBubble: true,
			noFlexShrinkWrap: true,
			zeroHeightActionAnchor: true,
			hoverBridgeCoversActionZone: true,
			hoverOnlyOnPointerDevices: true,
			focusKeepsActionsVisible: true,
		})
	})
})
