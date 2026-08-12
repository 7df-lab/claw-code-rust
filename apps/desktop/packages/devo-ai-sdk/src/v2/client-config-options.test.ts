import { describe, expect, test } from "bun:test"
import { createDevoClient, type DevoAcpTransport, type DevoAcpTransportEvent } from "./client"
import type { AcpSessionConfigOption, AcpSessionNotification } from "./generated"
import type {
	ProviderValidateParams,
	ProviderVendor,
	ProviderVendorUpsertParams,
} from "./generated/protocol"

class FakeTransport implements DevoAcpTransport {
	readonly requests: Array<{ method: string; params: unknown; directory?: string }> = []
	private listeners: Array<(event: DevoAcpTransportEvent) => void> = []

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

	subscribe(listener: (event: DevoAcpTransportEvent) => void): () => void {
		this.listeners.push(listener)
		return () => {
			this.listeners = this.listeners.filter((item) => item !== listener)
		}
	}

	connected(): boolean {
		return true
	}

	emitSessionUpdate(params: unknown): void {
		for (const listener of this.listeners) {
			listener({ type: "notification", method: "session/update", params })
		}
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
		{ value: "test-openai", label: "Test OpenAI", description: "OpenAI: test-model" },
		{ value: "alt-openai", label: "Alt OpenAI", description: "OpenAI: alt-model" },
	],
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
] satisfies AcpSessionConfigOption[]

const providerVendor = {
	name: "openai",
	base_url: "https://api.openai.com/v1",
	credential: "openai_api_key",
	headers: null,
	wire_apis: ["openai_chat_completions"],
	enabled: true,
} satisfies ProviderVendor

const canonicalProviderVendor = {
	name: "openai",
	baseUrl: "https://api.openai.com/v1",
	credential: "openai_api_key",
	wireApis: ["openai_chat_completions"],
	enabled: true,
}

const canonicalModelBinding = {
	bindingId: "openai-gpt-4o",
	modelSlug: "gpt-4o",
	provider: "openai",
	requestModel: "gpt-4o",
	displayName: "GPT-4o",
	invocationMethod: "openai_chat_completions",
	enabled: true,
}

const providerValidateParams = {
	provider_vendor: providerVendor,
	model_binding: {
		binding_id: "openai-gpt-4o",
		model_slug: "gpt-4o",
		provider: "openai",
		request_model: "gpt-4o",
		display_name: "GPT-4o",
		invocation_method: "openai_chat_completions",
		default_reasoning_effort: null,
		enabled: true,
	},
	api_key: "secret",
} satisfies ProviderValidateParams

const providerUpsertParams = {
	...providerValidateParams,
	default_model_binding: "openai-gpt-4o",
} satisfies ProviderVendorUpsertParams

describe("ACP desktop SDK config option cache", () => {
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
		expect(config.data).toEqual({ model: "session/test-openai" })
		expect(transport.requests.map((request) => request.method)).toEqual([
			"initialize",
			"model/preferences/read",
		])
	})

	test("keeps session model options when a live config update is empty", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/new") return { sessionId: "s1", configOptions }
			if (method === "model/preferences/read") {
				throw new Error("model/preferences/read should not be called")
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.create()
		const before = await client.config.providers()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "config_option_update",
				configOptions: [],
			},
		} satisfies AcpSessionNotification)
		const after = await client.config.providers()

		expect(after.data).toEqual(before.data)
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

	test("lists provider vendors through the server provider API", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "provider/list") {
				expect(params).toEqual({})
				return { providers: [canonicalProviderVendor] }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const result = await client.provider.list()

		expect(result.data).toEqual({ provider_vendors: [providerVendor] })
		expect(transport.requests.map((request) => request.method)).toEqual(["provider/list"])
	})

	test("validates provider candidates through the server provider API", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "provider/validate") {
				expect(params).toEqual({
					providerVendor: canonicalProviderVendor,
					modelBinding: canonicalModelBinding,
					apiKey: "secret",
				})
				return { replyPreview: "OK" }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const result = await client.provider.validate(providerValidateParams)

		expect(result.data).toEqual({ reply_preview: "OK" })
		expect(transport.requests.map((request) => request.method)).toEqual(["provider/validate"])
	})

	test("upserts provider vendors and clears cached model config", async () => {
		let modelPreferencesReadCalls = 0
		const updatedPreferences = {
			model: "openai-gpt-4o",
			availableModels: [
				{ value: "openai-gpt-4o", label: "GPT-4o", description: "OpenAI: gpt-4o" },
			],
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
				expect(params).toEqual({
					providerVendor: canonicalProviderVendor,
					modelBinding: canonicalModelBinding,
					defaultModelBinding: "openai-gpt-4o",
					apiKey: "secret",
				})
				return {
					providerVendor: canonicalProviderVendor,
					modelBinding: canonicalModelBinding,
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
