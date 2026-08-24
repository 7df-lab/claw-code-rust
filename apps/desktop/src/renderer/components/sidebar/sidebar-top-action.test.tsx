import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { TopActionRow, sidebarPrimaryIconClass } from "./sidebar-top-action"

describe("TopActionRow", () => {
	test("keeps the New chat / Search action treatment", () => {
		const markup = renderToStaticMarkup(
			<TopActionRow
				icon={<span className={sidebarPrimaryIconClass} />}
				onClick={() => {}}
				isActive
			>
				New chat
			</TopActionRow>,
		)

		expect({
			hasHeight: markup.includes("h-8"),
			hasTextSize: markup.includes("text-[13px]"),
			hasFontNormal: markup.includes("font-normal"),
			hasRounded: markup.includes("rounded-lg"),
			hasActiveBackground: markup.includes("bg-black/[0.06]"),
			hasIconClass: markup.includes("size-[15px] stroke-[1.5]"),
		}).toEqual({
			hasHeight: true,
			hasTextSize: true,
			hasFontNormal: true,
			hasRounded: true,
			hasActiveBackground: true,
			hasIconClass: true,
		})
	})
})
