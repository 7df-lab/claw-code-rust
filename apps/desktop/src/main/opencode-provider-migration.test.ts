import { describe, expect, test } from "bun:test"
import {
	buildOpenCodeProviderMigrationPreview,
	executeOpenCodeProviderMigration,
} from "./opencode-provider-migration"

function scanResultWithConfig(
	config?: Record<string, unknown>,
	auth?: Record<string, unknown>,
	parseErrors: string[] = [],
) {
	return {
		format: "opencode",
		data: {
			global: {
				config,
				auth,
				parseErrors,
			},
		},
	}
}

const exampleConfig = {
	model: "deepseek/deepseek-v4-pro",
	small_model: "deepseek/deepseek-v4-flash",
	provider: {
		deepseek: {
			npm: "@ai-sdk/openai-compatible",
			options: {
				baseURL: "https://api.deepseek.com/v1",
				apiKey: "sk-xxxx",
				setCacheKey: true,
			},
			models: {
				"deepseek-v4-flash": {
					name: "DeepSeek V4 Flash",
				},
				"deepseek-v4-pro": {
					name: "DeepSeek V4 Pro",
				},
			},
		},
		mimo: {
			npm: "@ai-sdk/openai-compatible",
			name: "Xiaomi MiMo",
			options: {
				baseURL: "https://token-plan-cn.xiaomimimo.com/v1",
				apiKey: "tp-xxxxxx",
				setCacheKey: true,
			},
			models: {
				"mimo-v2.5": {
					name: "Mimo v2.5",
				},
				"mimo-v2.5-pro": {
					name: "Mimo v2.5 Pro",
				},
			},
		},
	},
}

describe("OpenCode provider migration", () => {
	test("builds a sanitized provider preview without exposing API keys", () => {
		const result = buildOpenCodeProviderMigrationPreview(scanResultWithConfig(exampleConfig))

		expect(result.category?.category).toBe("config")
		expect(result.category?.files.map((file) => file.path)).toEqual([
			"provider/upsert:deepseek",
			"provider/upsert:mimo",
		])
		expect(result.category?.files[0].content).toContain("Provider ID: deepseek")
		expect(result.category?.files[0].content).toContain("Base URL: https://api.deepseek.com/v1")
		expect(result.category?.files[0].content).toContain("Wire API: openai_chat_completions")
		expect(result.category?.files[0].content).toContain("Default model: deepseek-v4-pro")
		expect(result.category?.files[0].content).toContain("API key: detected")
		expect(result.category?.files[0].content).not.toContain("sk-xxxx")
		expect(result.category?.files[1].content).not.toContain("tp-xxxxxx")
	})

	test("executes one canonical provider upsert per imported provider", async () => {
		const calls: Array<{ method: string; params: Record<string, unknown> }> = []

		const result = await executeOpenCodeProviderMigration(
			scanResultWithConfig(exampleConfig),
			async (method, params) => {
				calls.push({ method, params: params as Record<string, unknown> })
				return {}
			},
		)

		expect(result.errors).toEqual([])
		expect(result.filesWritten).toEqual(["provider/upsert:deepseek", "provider/upsert:mimo"])
		expect(calls.map((call) => call.method)).toEqual([
			"provider/upsert",
			"provider/upsert",
		])
		expect(calls[1].params).toEqual({
			provider: {
				id: "mimo",
				name: "Xiaomi MiMo",
				baseUrl: "https://token-plan-cn.xiaomimimo.com/v1",
				wireApis: ["openai_chat_completions"],
				models: {
					"mimo-v2.5": { name: "Mimo v2.5" },
					"mimo-v2.5-pro": { name: "Mimo v2.5 Pro" },
				},
				enabled: true,
			},
		apiKey: "tp-xxxxxx",
		})
		expect(calls[0].params.defaultModel).toBe("deepseek/deepseek-v4-pro")
		expect(calls[0].params.smallModel).toBe("deepseek/deepseek-v4-flash")
		expect(calls[0].params.apiKey).toBe("sk-xxxx")
		expect(calls[1].params.defaultModel).toBeUndefined()
		expect(calls[1].params.smallModel).toBeUndefined()
	})

	test("does not guess a default model when OpenCode does not define one", async () => {
		const calls: Array<Record<string, unknown>> = []

		await executeOpenCodeProviderMigration(
			scanResultWithConfig({
				provider: {
					deepseek: {
						npm: "@ai-sdk/openai-compatible",
						options: {
							baseURL: "https://api.deepseek.com/v1",
							apiKey: "sk-xxxx",
						},
						models: { "deepseek-v4-pro": { name: "DeepSeek V4 Pro" } },
					},
				},
			}),
			async (_method, params) => {
				calls.push(params as Record<string, unknown>)
				return {}
			},
		)

		expect(calls).toHaveLength(1)
		expect(calls[0].defaultModel).toBeUndefined()
	})

	test("reports missing config, parse errors, unsupported providers, and missing key/base URL without crashing", async () => {
		const missingConfig = buildOpenCodeProviderMigrationPreview(scanResultWithConfig())
		const calls: Array<Record<string, unknown>> = []

		const result = await executeOpenCodeProviderMigration(
			scanResultWithConfig(
				{
					provider: {
						anthropic: {
							npm: "@ai-sdk/anthropic",
							models: { "claude-sonnet-4-5": { name: "Claude Sonnet" } },
						},
						empty: {
							npm: "@ai-sdk/openai-compatible",
							options: {},
							models: { "empty-model": {} },
						},
					},
				},
				undefined,
				["opencode.json: JSONC parse error at offset 10"],
			),
			async (_method, params) => {
				calls.push(params as Record<string, unknown>)
				return {}
			},
		)

		expect(missingConfig.category).toBeNull()
		expect(missingConfig.warnings.some((warning) => warning.includes("opencode.json"))).toBe(true)
		expect(result.errors).toEqual([])
		expect(result.warnings.some((warning) => warning.includes("@ai-sdk/anthropic"))).toBe(true)
		expect(result.warnings.some((warning) => warning.includes("baseURL"))).toBe(true)
		expect(result.warnings.some((warning) => warning.includes("JSONC parse error"))).toBe(true)
		expect(result.manualActions.some((action) => action.includes("API key"))).toBe(true)
		expect(calls).toHaveLength(1)
		expect(calls[0].api_key).toBeUndefined()
	})
})
