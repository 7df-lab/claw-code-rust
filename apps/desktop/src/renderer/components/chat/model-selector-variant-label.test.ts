import { describe, expect, test } from "bun:test"
import {
	getVariantMenuLabel,
	getVariantTriggerLabel,
	resolveSelectedVariant,
} from "./model-selector-variant-label"

describe("model selector variant labels", () => {
	test("formats known reasoning strengths for trigger and menu display", () => {
		expect(getVariantTriggerLabel("max")).toBe("max")
		expect(getVariantMenuLabel("max")).toBe("Max")
	})

	test("formats disabled as off for user-facing reasoning strength labels", () => {
		expect(getVariantTriggerLabel("disabled")).toBe("off")
		expect(getVariantTriggerLabel("off")).toBe("off")
		expect(getVariantTriggerLabel("enabled")).toBe("on")
		expect(getVariantMenuLabel("disabled")).toBe("Off")
		expect(getVariantMenuLabel("on")).toBe("On")
	})

	test("keeps unknown variants stable", () => {
		expect(getVariantTriggerLabel("turbo-mode")).toBe("turbo-mode")
		expect(getVariantMenuLabel("turbo-mode")).toBe("Turbo-mode")
	})

	test("resolves stale or missing selections like the existing variant selector", () => {
		expect(resolveSelectedVariant(["low", "max"], "max", "low", true)).toBe("max")
		expect(resolveSelectedVariant(["low", "max"], undefined, "low", true)).toBe("low")
		expect(resolveSelectedVariant(["low", "max"], "stale", undefined, true)).toBeUndefined()
		expect(resolveSelectedVariant(["low", "max"], "stale", undefined, false)).toBe("low")
	})
})
