import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { ModelSelectorOptionRow } from "./model-selector-option-row"

function checkSlotClassLists(html: string): string[][] {
	return Array.from(
		html.matchAll(/<span[^>]*data-slot="model-selector-check-slot"[^>]*class="([^"]+)"/g),
		(match) => match[1].split(/\s+/),
	)
}

function firstButtonClassList(html: string): string[] {
	const match = html.match(/<button[^>]*class="([^"]+)"/)
	return match ? match[1].split(/\s+/) : []
}

describe("ModelSelectorOptionRow", () => {
	test("reserves the same leading check slot for selected and unselected rows", () => {
		const html = renderToStaticMarkup(
			<>
				<ModelSelectorOptionRow
					displayName="deepseek-v4-flash"
					reasoning
					selected
					onSelect={() => undefined}
				/>
				<ModelSelectorOptionRow
					displayName="deepseek-v4-pro"
					reasoning
					selected={false}
					onSelect={() => undefined}
				/>
			</>,
		)

		const slots = checkSlotClassLists(html)

		expect(slots).toEqual([
			["flex", "size-3.5", "shrink-0", "items-center", "justify-center"],
			["flex", "size-3.5", "shrink-0", "items-center", "justify-center"],
		])
	})

	test("renders the check icon only for the selected row", () => {
		const html = renderToStaticMarkup(
			<>
				<ModelSelectorOptionRow
					displayName="deepseek-v4-flash"
					reasoning
					selected
					onSelect={() => undefined}
				/>
				<ModelSelectorOptionRow
					displayName="deepseek-v4-pro"
					reasoning
					selected={false}
					onSelect={() => undefined}
				/>
			</>,
		)

		expect(html.match(/data-slot="model-selector-check-icon"/g)).toHaveLength(1)
	})

	test("uses a high-contrast hover and focus state", () => {
		const html = renderToStaticMarkup(
			<ModelSelectorOptionRow
				displayName="deepseek-v4-flash"
				reasoning
				selected={false}
				onSelect={() => undefined}
			/>,
		)

		expect(firstButtonClassList(html)).toEqual(
			expect.arrayContaining([
				"hover:bg-muted",
				"hover:text-foreground",
				"focus-visible:bg-muted",
				"focus-visible:text-foreground",
				"focus-visible:outline-none",
			]),
		)
		expect(firstButtonClassList(html)).not.toContain("hover:bg-accent")
	})

	test("does not render a reasoning badge for reasoning-capable models", () => {
		const html = renderToStaticMarkup(
			<ModelSelectorOptionRow
				displayName="deepseek-v4-flash"
				reasoning
				selected={false}
				onSelect={() => undefined}
			/>,
		)

		expect(html).not.toContain("reasoning")
	})
})
