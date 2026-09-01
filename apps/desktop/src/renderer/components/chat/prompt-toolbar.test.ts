import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./prompt-toolbar.tsx", import.meta.url), "utf8")
const modelSelectorProps = source.match(/interface ModelSelectorProps \{[\s\S]*?\n\}/)?.[0] ?? ""
const promptToolbarProps = source.match(/export interface PromptToolbarProps \{[\s\S]*?\n\}/)?.[0] ?? ""

describe("Model selector menu", () => {
	test("does not expose a Last used presentation group", () => {
		expect({
			omitsLastUsedHeading: !source.includes("Last used"),
			omitsLastUsedPresentationState: !source.includes("lastUsedModels"),
			modelSelectorPropRemoved: !modelSelectorProps.includes("recentModels"),
			promptToolbarPropRemoved: !promptToolbarProps.includes("recentModels"),
		}).toEqual({
			omitsLastUsedHeading: true,
			omitsLastUsedPresentationState: true,
			modelSelectorPropRemoved: true,
			promptToolbarPropRemoved: true,
		})
	})

	test("keeps provider grouping and active model selection", () => {
		expect({
			groupsFilteredModelsByProvider: source.includes("groupByProvider(models)"),
			rendersProviderGroups: source.includes("<SearchableListPopoverGroup"),
			keepsSessionModelsUngrouped: source.includes('providerId === "session"'),
			keepsActiveModelCheck: source.includes("selected={model.value === activeValue}"),
			omitsSearchField: !source.includes("SearchableListPopoverSearch") && !source.includes("Search models"),
		}).toEqual({
			groupsFilteredModelsByProvider: true,
			rendersProviderGroups: true,
			keepsSessionModelsUngrouped: true,
			keepsActiveModelCheck: true,
			omitsSearchField: true,
		})
	})
})
