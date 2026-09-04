import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoNativeTransport, type DevoNativeTransportEvent } from "./client"
import type { SessionConfigOption } from "./native-client-support"
import type {
	ProviderInfo,
	ProviderModelInfo,
	ProviderUpsertParams,
	ProviderValidateParams,
} from "./generated/native"

class FakeTransport implements DevoNativeTransport {
	readonly requests: Array<{ method: string; params: unknown; directory?: string }> = []
	private listeners: Array<(event: DevoNativeTransportEvent) => void> = []

	constructor(
		private readonly handler: (
			method: string,
			params: unknown,
			directory?: string,
		) => unknown,
	) {}

	async request(method: string, params?: unknown, directory?: string): Promise<unknown> {
		this.requests.push({ method, params, directory })
		return this.handler(method, params, directory)
	}

	async respond(): Promise<void> {}

	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void {
		this.listeners.push(listener)
		return () => {
			this.listeners = this.listeners.filter((item) => item !== listener)
		}
	}

	connected(): boolean {
		return true
	}

}

const initializeResult = {
	protocolVersion: 1,
	agentCapabilities: {},
	authMethods: [],
}

const modelPreferences = {
	model: "test-openai",
	availableModels: [
		{
			value: "test-openai",
			label: "Test OpenAI",
			description: "OpenAI: test-model",
			availableEfforts: [
				{ value: "low", label: "Low" },
				{ value: "medium", label: "Medium" },
				{ value: "high", label: "High" },
			],
		},
		{
			value: "alt-openai",
			label: "Alt OpenAI",
			description: "OpenAI: alt-model",
			availableEfforts: [
				{ value: "high", label: "High" },
				{ value: "max", label: "Max" },
			],
		},
	],
	availableEfforts: [
		{ value: "low", label: "Low" },
		{ value: "medium", label: "Medium" },
		{ value: "high", label: "High" },
	],
	reasoningEffort: "medium",
}

const configOptions = [
	{
		type: "select",
		id: "model",
		name: "Model",
		category: "model",
		currentValue: "test-openai",
		options: [
			{ value: "test-openai", name: "Test OpenAI", description: "OpenAI: test-model" },
			{ value: "alt-openai", name: "Alt OpenAI", description: "OpenAI: alt-model" },
		],
	},
] satisfies SessionConfigOption[]

const provider = {
	id: "openai",
	name: "openai",
	baseUrl: "https://api.openai.com/v1",
	credential: "openai_api_key",
	wireApis: ["openai_chat_completions"],
	models: {
		"gpt-4o": {
			name: "GPT-4o",
			wireApi: "openai_chat_completions",
		} satisfies ProviderModelInfo,
	},
	enabled: true,
} satisfies ProviderInfo

const providerValidateParams = {
	provider,
	model: "gpt-4o",
	apiKey: "secret",
} satisfies ProviderValidateParams

const providerUpsertParams = {
	...providerValidateParams,
	defaultModel: "openai/gpt-4o",
} satisfies ProviderUpsertParams

describe("Native desktop SDK config option cache", () => {
	test("loads cold-start config options from model/preferences/read when no session cache exists", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "model/preferences/read") {
				expect(params).toEqual({ cwd: "/repo" })
				return { preferences: modelPreferences }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const providers = await client.config.providers()
		const config = await client.config.get()

		expect(providers.data.default).toEqual({ session: "test-openai" })
		expect(Object.keys(providers.data.providers[0].models["test-openai"].variants)).toEqual([
			"low",
			"medium",
			"high",
		])
		expect(Object.keys(providers.data.providers[0].models["alt-openai"].variants)).toEqual([
			"high",
			"max",
		])
		expect(providers.data.providers[0].models["test-openai"].currentVariant).toBe("medium")
		expect(providers.data.providers[0].models["alt-openai"].currentVariant).toBeUndefined()
		expect(config.data).toEqual({ model: "session/test-openai" })
		expect(transport.requests.map((request) => request.method)).toEqual([
			"initialize",
			"model/preferences/read",
		])
	})

	test("persists cold-start model config options through the runtime API", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "model/preferences/write") {
				expect(params).toEqual({
					cwd: "/repo",
					patch: { model: "alt-openai" },
				})
				return { preferences: { ...modelPreferences, model: "alt-openai" } }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const result = await client.config.setOption({ configID: "model", value: "alt-openai" })

		expect(result.data).toEqual({ model: "session/alt-openai" })
		expect((await client.config.get()).data).toEqual({ model: "session/alt-openai" })
		expect(transport.requests.map((request) => request.method)).toEqual([
			"initialize",
			"model/preferences/write",
		])
	})

	test("lists provider Connections and templates through the server provider API", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "provider/list") {
				expect(params).toEqual({})
				return {
					providers: [provider],
					templateProviderIds: [],
					connectedProviderIds: ["openai"],
					connectionModels: { openai: { "gpt-4o": provider.models["gpt-4o"] } },
				}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const result = await client.provider.list()

		expect(result.data).toEqual({
			providers: [provider],
			templateProviderIds: [],
			connectedProviderIds: ["openai"],
			connectionModels: { openai: { "gpt-4o": provider.models["gpt-4o"] } },
		})
		expect(transport.requests.map((request) => request.method)).toEqual(["initialize", "provider/list"])
	})

	test("validates provider candidates through the server provider API", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "provider/validate") {
				expect(params).toEqual(providerValidateParams)
				return { replyPreview: "OK" }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const result = await client.provider.validate(providerValidateParams)

		expect(result.data).toEqual({ replyPreview: "OK" })
		expect(transport.requests.map((request) => request.method)).toEqual(["initialize", "provider/validate"])
	})

	test("upserts a provider Connection and clears cached model config", async () => {
		let modelPreferencesReadCalls = 0
		const updatedPreferences = {
			model: "openai-gpt-4o",
			availableModels: [
				{ value: "openai-gpt-4o", label: "GPT-4o", description: "OpenAI: gpt-4o" },
			],
			availableEfforts: [],
		}
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "model/preferences/read") {
				modelPreferencesReadCalls += 1
				return {
					preferences:
						modelPreferencesReadCalls === 1 ? modelPreferences : updatedPreferences,
				}
			}
			if (method === "provider/upsert") {
				expect(params).toEqual(providerUpsertParams)
				return {
					provider,
					defaultModel: "openai/gpt-4o",
				}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		expect((await client.config.get()).data).toEqual({ model: "session/test-openai" })
		await client.provider.upsert(providerUpsertParams)

		expect((await client.config.get()).data).toEqual({ model: "session/openai-gpt-4o" })
		expect(transport.requests.map((request) => request.method)).toEqual([
			"initialize",
			"model/preferences/read",
			"provider/upsert",
			"model/preferences/read",
		])
	})
})
