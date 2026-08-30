import { describe, expect, test } from "bun:test"
import { modelRefFromSlug } from "./use-devo-data"
import type { SdkProvider } from "./use-devo-data"

const providers: SdkProvider[] = [
	{
		id: "session",
		name: "session",
		models: {
			"deepseek-v4-flash": { name: "DeepSeek V4 Flash" },
			"ollama-qwen3.6-35b-a3b": { name: "Qwen 3.6 35B" },
		},
	} as unknown as SdkProvider,
]

describe("modelRefFromSlug", () => {
	test("exact config model id resolves directly", () => {
		expect(modelRefFromSlug("deepseek-v4-flash", providers)).toEqual({
			providerID: "session",
			modelID: "deepseek-v4-flash",
		})
	})

	test("resolved request slug falls back to the dash-boundary prefix", () => {
		// Sessions whose last state came from a turn report
		// `<config-id>-<provider-name>` as the model slug.
		expect(modelRefFromSlug("deepseek-v4-flash-deepseek-ac", providers)).toEqual({
			providerID: "session",
			modelID: "deepseek-v4-flash",
		})
	})

	test("unresolvable slug returns null", () => {
		expect(modelRefFromSlug("unknown-model", providers)).toBeNull()
		expect(modelRefFromSlug("", providers)).toBeNull()
	})

	test("longest prefix wins when one id prefixes another", () => {
		const nested = [
			{
				id: "session",
				name: "session",
				models: {
					model: { name: "Model" },
					"model-pro": { name: "Model Pro" },
				},
			},
		] as unknown as SdkProvider[]
		expect(modelRefFromSlug("model-pro-vendor", nested)?.modelID).toBe("model-pro")
	})

	test("provider-prefixed slug falls back to the dash-boundary suffix", () => {
		// Historically the persisted slug could carry a provider prefix the
		// configured id does not start with (e.g. an ollama-exposed model).
		const prefixed = [
			{
				id: "session",
				name: "session",
				models: {
					"qwen3.6-35b-a3b": { name: "Qwen 3.6 35B" },
				},
			},
		] as unknown as SdkProvider[]
		expect(modelRefFromSlug("ollama-qwen3.6-35b-a3b", prefixed)).toEqual({
			providerID: "session",
			modelID: "qwen3.6-35b-a3b",
		})
	})

	test("longest suffix wins when one id suffixes another", () => {
		const nested = [
			{
				id: "session",
				name: "session",
				models: {
					"35b-a3b": { name: "35B" },
					"qwen3.6-35b-a3b": { name: "Qwen 3.6 35B" },
				},
			},
		] as unknown as SdkProvider[]
		expect(modelRefFromSlug("ollama-qwen3.6-35b-a3b", nested)?.modelID).toBe("qwen3.6-35b-a3b")
	})

	test("suffix match requires the full dash boundary", () => {
		const boundary = [
			{
				id: "session",
				name: "session",
				models: {
					"5b-a3b": { name: "5B" },
				},
			},
		] as unknown as SdkProvider[]
		// "35b-a3b" does not end with "-5b-a3b" — no dash boundary, no match.
		expect(modelRefFromSlug("ollama-qwen3.6-35b-a3b", boundary)).toBeNull()
	})
})
