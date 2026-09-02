import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import { ModelSelectorTriggerLabel } from "./model-selector-trigger-label"

function slotClassList(html: string, slot: string): string[] {
	const match = html.match(new RegExp(`data-slot="${slot}" class="([^"]+)"`))
	return match ? match[1].split(/\s+/) : []
}

describe("ModelSelectorTriggerLabel", () => {
	test("renders the model name and reasoning strength in quiet 13px type", () => {
		const html = renderToStaticMarkup(
			<ModelSelectorTriggerLabel displayName="deepseek-v4-flash" variantLabel="max" />,
		)

		expect(slotClassList(html, "model-selector-trigger-model")).toEqual(
			expect.arrayContaining(["text-[13px]", "font-normal", "text-muted-foreground"]),
		)
		expect(slotClassList(html, "model-selector-trigger-variant")).toEqual(
			expect.arrayContaining(["text-[12px]", "font-normal", "text-muted-foreground/50"]),
		)
		expect(html).toContain("deepseek-v4-flash")
		expect(html).toContain("max")
	})

	test("omits the variant suffix when no variant label is available", () => {
		const html = renderToStaticMarkup(
			<ModelSelectorTriggerLabel displayName="deepseek-v4-flash" />,
		)

		expect(html).toContain("deepseek-v4-flash")
		expect(html).not.toContain("model-selector-trigger-variant")
	})
})
