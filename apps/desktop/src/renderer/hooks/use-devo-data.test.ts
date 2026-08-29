import { describe, expect, test } from "bun:test"
import {
	getModelCurrentVariant,
	getModelVariants,
	loadVcsData,
	modelAllowsDefaultVariant,
	resolveEffectiveModel,
} from "./use-devo-data"

type Providers = Parameters<typeof getModelCurrentVariant>[2]

describe("model variant metadata", () => {
	test("reads current reasoning variant and disables default variant sentinel", () => {
		const providers = [
			{
				id: "session",
				models: {
					"test-openai": {
						variants: {
							disabled: { name: "Disabled" },
							high: { name: "High" },
						},
						currentVariant: "disabled",
						allowDefaultVariant: false,
					},
				},
			},
		] as Providers

		expect(getModelCurrentVariant("session", "test-openai", providers)).toBe("disabled")
		expect(modelAllowsDefaultVariant("session", "test-openai", providers)).toBe(false)
	})

	test("returns per-model reasoning effort variants", () => {
		const providers = [
			{
				id: "session",
				models: {
					"model-a": {
						variants: {
							r1: { name: "R1" },
							r2: { name: "R2" },
						},
						allowDefaultVariant: false,
					},
					"model-b": {
						variants: {
							t1: { name: "T1" },
							t2: { name: "T2" },
							t3: { name: "T3" },
						},
						allowDefaultVariant: false,
					},
				},
			},
		] as Providers

		expect(getModelVariants("session", "model-a", providers)).toEqual(["r1", "r2"])
		expect(getModelVariants("session", "model-b", providers)).toEqual(["t1", "t2", "t3"])
	})
})

describe("effective model resolution", () => {
	const providers = [
		{
			id: "session",
			models: {
				"test-openai": { name: "Test OpenAI" },
				"alt-openai": { name: "Alt OpenAI" },
			},
		},
	] as Providers

	test("uses the provider default when there is no explicit or recent model", () => {
		expect(resolveEffectiveModel(null, null, undefined, { session: "test-openai" }, providers)).toEqual({
			providerID: "session",
			modelID: "test-openai",
		})
	})

	test("keeps explicit and recent model precedence over provider defaults", () => {
		expect(
			resolveEffectiveModel(
				{ providerID: "session", modelID: "alt-openai" },
				null,
				undefined,
				{ session: "test-openai" },
				providers,
				[{ providerID: "session", modelID: "test-openai" }],
			),
		).toEqual({ providerID: "session", modelID: "alt-openai" })

		expect(
			resolveEffectiveModel(
				null,
				null,
				undefined,
				{ session: "test-openai" },
				providers,
				[{ providerID: "session", modelID: "alt-openai" }],
			),
		).toEqual({ providerID: "session", modelID: "alt-openai" })
	})
})

describe("VCS data loading", () => {
	test("uses Electron git branch data for desktop branch display", async () => {
		const result = await loadVcsData("/Users/tsiao/Desktop/devo_feat_desktop", {
			isElectron: true,
			fetchBranches: async () => ({
				state: "branch",
				current: "feat/desktop",
				detached: false,
				local: ["feat/desktop"],
				remote: [],
			}),
			getClient: () => {
				throw new Error("SDK vcs.get should not be used in Electron")
			},
		})

		expect(result).toEqual({ branch: "feat/desktop", state: "branch", detached: false })
	})

	test("handles missing SDK VCS data without hiding the rest of the toolbar", async () => {
		const result = await loadVcsData("/repo", {
			isElectron: false,
			getClient: () =>
				({
					vcs: {
						get: async () => ({ data: null }),
					},
				}) as never,
		})

		expect(result).toEqual({ branch: "", state: "not_git", detached: false })
	})
})
