import { describe, expect, test } from "bun:test"
import {
	projectMenuContentClass,
	rowMenuItemClass,
	sessionMenuContentClass,
} from "./sidebar-menu-styles"

function classList(className: string): string[] {
	return className.split(/\s+/)
}

describe("sidebar menu styles", () => {
	test("uses quiet muted hover and focus states", () => {
		expect(classList(rowMenuItemClass)).toEqual(
			expect.arrayContaining([
				"hover:bg-muted",
				"focus:bg-muted",
				"data-[highlighted]:bg-muted",
			]),
		)
		expect(classList(rowMenuItemClass)).not.toContain("dark:focus:bg-white/[0.08]")
	})

	test("keeps project menus wide and narrows session action menus", () => {
		expect(classList(projectMenuContentClass)).toContain("w-[232px]")
		expect(classList(sessionMenuContentClass)).toContain("w-44")
		expect(classList(sessionMenuContentClass)).not.toContain("w-[232px]")
	})
})
