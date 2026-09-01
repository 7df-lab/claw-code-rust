import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"

const popoverSource = readFileSync(new URL("./composer-popover.tsx", import.meta.url), "utf8")
const mentionSource = readFileSync(new URL("./mention-popover.tsx", import.meta.url), "utf8")
const slashSource = readFileSync(new URL("./slash-command-popover.tsx", import.meta.url), "utf8")

describe("shared composer popover", () => {
	test("is the list surface for both @ and / suggestions", () => {
		expect({
			mentionUsesShared: mentionSource.includes('from "./composer-popover"'),
			slashUsesShared: slashSource.includes('from "./composer-popover"'),
			noSearchField:
				!popoverSource.includes("SearchIcon") &&
				!popoverSource.includes("<input") &&
				!popoverSource.includes("placeholder"),
			usesOptionMenuChrome:
				popoverSource.includes("optionMenuContentClass") &&
				popoverSource.includes("optionMenuItemClass"),
			activeIsMuted: popoverSource.includes("bg-muted") && !popoverSource.includes("bg-accent"),
			iconsMatchSidebar:
				popoverSource.includes("size-3.5") && popoverSource.includes("stroke-[1.5]"),
			singleScroll:
				popoverSource.includes("overflow-y-auto") &&
				!popoverSource.includes("@devo/ui/components/scroll-area"),
		}).toEqual({
			mentionUsesShared: true,
			slashUsesShared: true,
			noSearchField: true,
			usesOptionMenuChrome: true,
			activeIsMuted: true,
			iconsMatchSidebar: true,
			singleScroll: true,
		})
	})
})
