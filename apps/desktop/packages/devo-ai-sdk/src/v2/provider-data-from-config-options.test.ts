import { describe, expect, test } from "bun:test"
import { providerDataFromConfigOptions, type SessionConfigOption } from "./native-client-support"

describe("providerDataFromConfigOptions per-model efforts", () => {
	test("projects each model's availableEfforts into its own variants", () => {
		const configOptions = [
			{
				type: "select",
				id: "model",
				name: "Model",
				category: "model",
				currentValue: "model-a",
				options: [
					{
						value: "model-a",
						name: "Model A",
						availableEfforts: [
							{ value: "r1", name: "R1" },
							{ value: "r2", name: "R2" },
						],
					},
					{
						value: "model-b",
						name: "Model B",
						availableEfforts: [
							{ value: "t1", name: "T1" },
							{ value: "t2", name: "T2" },
							{ value: "t3", name: "T3" },
						],
					},
				],
			},
			{
				type: "select",
				id: "thought_level",
				name: "Reasoning Effort",
				category: "thought_level",
				currentValue: "r1",
				options: [
					{ value: "r1", name: "R1" },
					{ value: "r2", name: "R2" },
				],
			},
		] satisfies SessionConfigOption[]

		const data = providerDataFromConfigOptions(configOptions)
		const models = data.providers[0]?.models as Record<string, any>

		expect(Object.keys(models["model-a"].variants)).toEqual(["r1", "r2"])
		expect(Object.keys(models["model-b"].variants)).toEqual(["t1", "t2", "t3"])
		expect(models["model-a"].currentVariant).toBe("r1")
		expect(models["model-b"].currentVariant).toBeUndefined()
		expect(models["model-a"].allowDefaultVariant).toBe(false)
		expect(models["model-b"].allowDefaultVariant).toBe(false)
	})

	test("falls back to global thought_level when a model has no availableEfforts", () => {
		const configOptions = [
			{
				type: "select",
				id: "model",
				name: "Model",
				category: "model",
				currentValue: "legacy-model",
				options: [{ value: "legacy-model", name: "Legacy" }],
			},
			{
				type: "select",
				id: "thought_level",
				name: "Reasoning Effort",
				category: "thought_level",
				currentValue: "high",
				options: [
					{ value: "low", name: "Low" },
					{ value: "high", name: "High" },
				],
			},
		] satisfies SessionConfigOption[]

		const data = providerDataFromConfigOptions(configOptions)
		const models = data.providers[0]?.models as Record<string, any>

		expect(Object.keys(models["legacy-model"].variants)).toEqual(["low", "high"])
		expect(models["legacy-model"].currentVariant).toBe("high")
	})
})
