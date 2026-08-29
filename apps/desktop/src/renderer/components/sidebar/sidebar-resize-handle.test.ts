import { describe, expect, test } from "bun:test"
import {
	clampSidebarWidth,
	SIDEBAR_DEFAULT_WIDTH_PX,
	SIDEBAR_MAX_WIDTH_PX,
	SIDEBAR_MIN_WIDTH_PX,
} from "../../lib/sidebar-width"

describe("clampSidebarWidth", () => {
	test("clamps to min and max defaults", () => {
		expect({
			belowMin: clampSidebarWidth(100),
			aboveMax: clampSidebarWidth(900),
			defaultPassthrough: clampSidebarWidth(SIDEBAR_DEFAULT_WIDTH_PX),
		}).toEqual({
			belowMin: SIDEBAR_MIN_WIDTH_PX,
			aboveMax: SIDEBAR_MAX_WIDTH_PX,
			defaultPassthrough: SIDEBAR_DEFAULT_WIDTH_PX,
		})
	})

	test("reserves room for the main content pane", () => {
		expect(clampSidebarWidth(480, { windowWidth: 700, contentMinWidth: 360 })).toEqual(340)
	})

	test("falls back for non-finite values", () => {
		expect(clampSidebarWidth(Number.NaN)).toEqual(SIDEBAR_DEFAULT_WIDTH_PX)
	})
})
