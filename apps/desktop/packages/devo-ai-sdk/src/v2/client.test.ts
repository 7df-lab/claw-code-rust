import { afterEach, describe, expect, test } from "bun:test"
import { createDevoClient, type DevoAcpTransport, type DevoAcpTransportEvent } from "./client"
import type {
	AcpRequestPermissionParams,
	AcpSessionConfigOption,
	AcpSessionInfo,
	AcpSessionNotification,
} from "./generated"

class FakeTransport implements DevoAcpTransport {
	readonly requests: Array<{ method: string; params: unknown; directory?: string }> = []
	readonly responses: Array<{ id: string | number; result: unknown }> = []
	private listeners: Array<(event: DevoAcpTransportEvent) => void> = []

	constructor(
		private readonly handler: (
			method: string,
			params: unknown,
			directory?: string,
			transport?: FakeTransport,
		) => unknown,
	) {}

	async request(method: string, params?: unknown, directory?: string): Promise<unknown> {
		this.requests.push({ method, params, directory })
		return this.handler(method, params, directory, this)
	}

	async respond(id: string | number, result: unknown): Promise<void> {
		this.responses.push({ id, result })
	}

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

	emitNotification(method: string, params: unknown): void {
		for (const listener of this.listeners) {
			listener({ type: "notification", method, params })
		}
	}

	emitRequest(id: string | number, method: string, params: unknown): void {
		for (const listener of this.listeners) {
			listener({ type: "request", id, method, params })
		}
	}
}

const sessionInfo = {
	sessionId: "s1",
	cwd: "/repo",
	title: "Existing session",
	updatedAt: "2026-06-24T00:00:00.000Z",
} satisfies AcpSessionInfo

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

const initializeResult = {
	protocolVersion: 1,
	agentCapabilities: {},
	authMethods: [],
}

const workspaceChangeView = {
	scope: "turn",
	status: "ready",
	workspace_root: "/repo",
	base: {
		kind: "turn_checkpoint",
		turn_id: "t1",
		checkpoint_id: "checkpoint-1",
		backend: "git_ghost_commit",
	},
	coverage: "git_visible",
	attribution: "workspace_net",
	change_set_status: "finalized",
	files: [
		{
			path: "src/main.rs",
			status: "modified",
			additions: 2,
			deletions: 1,
			binary: false,
			diff_truncated: false,
		},
	],
	stats: { files_changed: 1, additions: 2, deletions: 1 },
	unified_diff:
		"diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1 +1 @@\n-old\n+new\n",
	warnings: [],
	generated_at: "2026-06-26T00:00:00Z",
}

const originalNow = Date.now

afterEach(() => {
	Date.now = originalNow
})

async function nextPayload(stream: AsyncIterator<any>, label: string): Promise<any> {
	const result = await Promise.race([
		stream.next(),
		new Promise<IteratorResult<any>>((resolve) =>
			setTimeout(() => resolve({ value: { payload: { type: `timeout:${label}` } }, done: false }), 20),
		),
	])
	return result.value.payload
}

async function nextPayloadOfType(
	stream: AsyncIterator<any>,
	type: string,
	label: string,
): Promise<any> {
	const deadline = Date.now() + 100
	while (Date.now() < deadline) {
		const payload = await nextPayload(stream, label)
		if (payload?.type === type) return payload
		if (typeof payload?.type === "string" && payload.type.startsWith("timeout:")) return payload
	}
	return { type: `timeout:${label}` }
}

describe("ACP desktop SDK session mapping", () => {
	test("aborts a session through the Devo session interrupt extension", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/interrupt") return { interrupted: true }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.abort({ sessionID: "s1" })

		expect(transport.requests.map((request) => request.method)).toEqual([
			"session/interrupt",
		])
		expect(transport.requests.at(-1)).toEqual({
			method: "session/interrupt",
			directory: "/repo",
			params: {
				scope: { scope: "session", sessionId: "s1" },
			},
		})
	})

	test("loads ACP history and returns grouped messages with accumulated text parts", async () => {
		Date.now = () => 1_772_000_000_000
		const transport = new FakeTransport((method, _params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") {
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "u1",
						content: { type: "text", text: "hello" },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "agent_message_chunk",
						messageId: "a1",
						content: { type: "text", text: "hi " },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "agent_message_chunk",
						messageId: "a1",
						content: { type: "text", text: "there" },
					},
				} satisfies AcpSessionNotification)
				return { configOptions }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		const result = await client.session.messages({ sessionID: "s1" })

		expect(transport.requests.map((request) => request.method)).toEqual([
			"initialize",
			"session/list",
			"session/load",
		])
		expect(result.data).toEqual([
			{
				info: {
					id: "u1",
					sessionID: "s1",
					role: "user",
					time: { created: 1_772_000_000_000 },
				},
				parts: [
					{
						id: "u1-text",
						sessionID: "s1",
						messageID: "u1",
						type: "text",
						text: "hello",
						time: { start: 1_772_000_000_000 },
					},
				],
			},
			{
				info: {
					id: "a1",
					sessionID: "s1",
					role: "assistant",
					parentID: "u1",
					time: { created: 1_772_000_000_001 },
				},
				parts: [
					{
						id: "a1-text",
						sessionID: "s1",
						messageID: "a1",
						type: "text",
						text: "hi there",
						time: { start: 1_772_000_000_001 },
					},
				],
			},
		])
	})

	test("normalizes reasoning and tool parts for renderer time/output assumptions", async () => {
		Date.now = () => 1_772_000_000_000
		const transport = new FakeTransport((method, _params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") {
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "u1",
						content: { type: "text", text: "hello" },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "agent_thought_chunk",
						messageId: "a1",
						content: { type: "text", text: "thinking" },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "tool_call_update",
						toolCallId: "tool1",
						title: "Read file",
					},
				} satisfies AcpSessionNotification)
				return { configOptions }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		const result = await client.session.messages({ sessionID: "s1" })

		expect(result.data.at(-2).parts[0]).toEqual({
			id: "a1-reasoning",
			sessionID: "s1",
			messageID: "a1",
			type: "reasoning",
			text: "thinking",
			time: { start: 1_772_000_000_001 },
		})
		expect(result.data.at(-1).parts[0]).toEqual({
			id: "tool-tool1-part",
			sessionID: "s1",
			messageID: "tool-tool1",
			type: "tool",
			callID: "tool1",
			tool: "read",
			state: {
				status: "completed",
				input: {},
				output: "",
				title: "Read file",
				metadata: {},
				time: { start: 1_772_000_000_002, end: 1_772_000_000_002 },
			},
		})
	})

	test("uses ACP metadata to keep historical tools attached to their turns", async () => {
		Date.now = () => 1_772_000_000_000
		const transport = new FakeTransport((method, _params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") {
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "history-0",
						content: { type: "text", text: "first prompt" },
						_meta: { "devo/historyIndex": 0 },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "tool_call",
						toolCallId: "read-real-a",
						title: "Read A",
						kind: "read",
						status: "pending",
						_meta: { "devo/historyIndex": 1, "devo/parentMessageId": "history-0" },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "tool_call_update",
						toolCallId: "read-real-a",
						title: "Read A",
						status: "completed",
						rawOutput: "A done",
						_meta: { "devo/historyIndex": 2, "devo/parentMessageId": "history-0" },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "agent_message_chunk",
						messageId: "history-3",
						content: { type: "text", text: "first answer" },
						_meta: { "devo/historyIndex": 3, "devo/parentMessageId": "history-0" },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "history-4",
						content: { type: "text", text: "second prompt" },
						_meta: { "devo/historyIndex": 4 },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "tool_call_update",
						toolCallId: "read-real-b",
						title: "Read B",
						status: "completed",
						rawOutput: "B done",
						_meta: { "devo/historyIndex": 5, "devo/parentMessageId": "history-4" },
					},
				} satisfies AcpSessionNotification)
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "agent_message_chunk",
						messageId: "history-6",
						content: { type: "text", text: "second answer" },
						_meta: { "devo/historyIndex": 6, "devo/parentMessageId": "history-4" },
					},
				} satisfies AcpSessionNotification)
				return { configOptions }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		const result = await client.session.messages({ sessionID: "s1" })

		expect(
			result.data.map((entry) => ({
				id: entry.info.id,
				role: entry.info.role,
				parentID: entry.info.parentID,
				created: entry.info.time.created,
				parts: entry.parts.map((part) => ({
					type: part.type,
					text: part.text,
					callID: part.callID,
					output: part.state?.output,
				})),
			})),
		).toEqual([
			{
				id: "history-0",
				role: "user",
				parentID: undefined,
				created: 1,
				parts: [{ type: "text", text: "first prompt", callID: undefined, output: undefined }],
			},
			{
				id: "tool-read-real-a",
				role: "assistant",
				parentID: "history-0",
				created: 2,
				parts: [{ type: "tool", text: undefined, callID: "read-real-a", output: "A done" }],
			},
			{
				id: "history-3",
				role: "assistant",
				parentID: "history-0",
				created: 4,
				parts: [{ type: "text", text: "first answer", callID: undefined, output: undefined }],
			},
			{
				id: "history-4",
				role: "user",
				parentID: undefined,
				created: 5,
				parts: [{ type: "text", text: "second prompt", callID: undefined, output: undefined }],
			},
			{
				id: "tool-read-real-b",
				role: "assistant",
				parentID: "history-4",
				created: 6,
				parts: [{ type: "tool", text: undefined, callID: "read-real-b", output: "B done" }],
			},
			{
				id: "history-6",
				role: "assistant",
				parentID: "history-4",
				created: 7,
				parts: [{ type: "text", text: "second answer", callID: undefined, output: undefined }],
			},
		])
	})

	test("reparents same-turn assistant updates when the user echo arrives late", async () => {
		Date.now = () => 1_772_000_000_000
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") return { configOptions }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		await client.session.messages({ sessionID: "s1" })
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "agent_message_chunk",
				messageId: "assistant-live",
				content: { type: "text", text: "late answer" },
				_meta: { "devo/turnId": "turn-live", "devo/itemId": "assistant-live" },
			},
		} satisfies AcpSessionNotification)
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "tool_call",
				toolCallId: "tool-live",
				title: "Read live",
				kind: "read",
				status: "pending",
				_meta: { "devo/turnId": "turn-live", "devo/itemId": "tool-live-item" },
			},
		} satisfies AcpSessionNotification)
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "user_message_chunk",
				messageId: "user-live",
				content: { type: "text", text: "new prompt" },
				_meta: { "devo/turnId": "turn-live", "devo/itemId": "user-live" },
			},
		} satisfies AcpSessionNotification)

		const result = await client.session.messages({ sessionID: "s1" })

		expect(
			result.data.map((entry) => ({
				id: entry.info.id,
				role: entry.info.role,
				parentID: entry.info.parentID,
				created: entry.info.time.created,
			})),
		).toEqual([
			{ id: "user-live", role: "user", parentID: undefined, created: 1_771_999_999_999 },
			{ id: "assistant-live", role: "assistant", parentID: "user-live", created: 1_772_000_000_000 },
			{ id: "tool-tool-live", role: "assistant", parentID: "user-live", created: 1_772_000_000_001 },
		])
	})
	test("uses ACP config options for model list and applies selected model before prompting", async () => {
		const transport = new FakeTransport((method, _params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") {
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "u1",
						content: { type: "text", text: "hello" },
					},
				} satisfies AcpSessionNotification)
				return { configOptions }
			}
			if (method === "session/set_config_option") return { configOptions }
			if (method === "session/prompt") return { stopReason: "end_turn" }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		await client.session.messages({ sessionID: "s1" })
		const providers = await client.config.providers()
		await client.session.promptAsync({
			sessionID: "s1",
			parts: [{ type: "text", text: "use another model" }],
			model: { providerID: "session", modelID: "alt-openai" },
		})

		expect(providers.data).toEqual({
			default: { session: "test-openai" },
			providers: [
				{
					id: "session",
					name: "Session",
					models: {
						"test-openai": {
							name: "Test OpenAI",
							description: "OpenAI: test-model",
							capabilities: {
								reasoning: false,
								input: { image: false, pdf: false },
								attachment: false,
							},
						},
						"alt-openai": {
							name: "Alt OpenAI",
							description: "OpenAI: alt-model",
							capabilities: {
								reasoning: false,
								input: { image: false, pdf: false },
								attachment: false,
							},
						},
					},
				},
			],
		})
		expect(transport.requests.at(-2)).toEqual({
			method: "session/set_config_option",
			directory: "/repo",
			params: { sessionId: "s1", configId: "model", value: "alt-openai" },
		})
		expect(transport.requests.at(-1)).toEqual({
			method: "session/prompt",
			directory: "/repo",
			params: {
				sessionId: "s1",
				prompt: [{ type: "text", text: "use another model" }],
			},
		})
	})

	test("turn start sends the selected model binding without a legacy model", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/set_config_option") return { configOptions }
			if (method === "turn/start") {
				return {
					disposition: "started",
					turn_id: "00000000-0000-0000-0000-000000000001",
					status: "Running",
					accepted_at: "2026-07-13T00:00:00Z",
				}
			}
			throw new Error("unexpected request " + method)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.turn.start({
			sessionID: "s1",
			parts: [{ type: "text", text: "plan this" }],
			model: { providerID: "session", modelID: "glm-zai" },
			collaborationMode: "plan",
		})

		expect(transport.requests.at(-2)).toEqual({
			method: "session/set_config_option",
			directory: "/repo",
			params: { sessionId: "s1", configId: "model", value: "glm-zai" },
		})
		expect(transport.requests.at(-1)).toEqual({
			method: "turn/start",
			directory: "/repo",
			params: {
				session_id: "s1",
				input: [{ type: "text", text: "plan this" }],
				model_binding_id: "glm-zai",
				sandbox: null,
				approval_policy: null,
				cwd: null,
				collaboration_mode: "plan",
			},
		})
	})

	test("starts ACP prompt without waiting for the final turn response", async () => {
		const transport = new FakeTransport((method, _params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") {
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "u1",
						content: { type: "text", text: "hello" },
					},
				} satisfies AcpSessionNotification)
				return { configOptions }
			}
			if (method === "session/prompt") return new Promise(() => {})
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		await client.session.messages({ sessionID: "s1" })
		const result = await Promise.race([
			client.session
				.promptAsync({
					sessionID: "s1",
					parts: [{ type: "text", text: "stream now" }],
				})
				.then(() => "returned"),
			new Promise((resolve) => setTimeout(() => resolve("pending"), 10)),
		])

		expect(result).toBe("returned")
		expect(transport.requests.at(-1)).toEqual({
			method: "session/prompt",
			directory: "/repo",
			params: {
				sessionId: "s1",
				prompt: [{ type: "text", text: "stream now" }],
			},
		})
	})

	test("emits busy and idle status around detached ACP prompt lifecycle", async () => {
		let resolvePrompt: (value: unknown) => void = () => {}
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/prompt") {
				return new Promise((resolve) => {
					resolvePrompt = resolve
				})
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		await client.session.promptAsync({
			sessionID: "s1",
			parts: [{ type: "text", text: "stream now" }],
		})

		expect(await nextPayload(stream, "busy")).toEqual({
			type: "session.status",
			properties: {
				sessionID: "s1",
				status: { type: "busy" },
			},
		})

		resolvePrompt({ stopReason: "end_turn" })

		expect(await nextPayload(stream, "idle")).toEqual({
			type: "session.status",
			properties: {
				sessionID: "s1",
				status: { type: "idle" },
			},
		})
	})

	test("marks streamed assistant messages completed when detached ACP prompt resolves", async () => {
		Date.now = () => 1_772_000_000_000
		const transport = new FakeTransport((method, _params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/prompt") {
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "agent_message_chunk",
						messageId: "a1",
						content: { type: "text", text: "done" },
					},
				} satisfies AcpSessionNotification)
				return { stopReason: "end_turn" }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		await client.session.promptAsync({
			sessionID: "s1",
			parts: [{ type: "text", text: "stream now" }],
		})

		const payloads = []
		for (let i = 0; i < 6; i++) {
			payloads.push(await nextPayload(stream, `completion-${i}`))
		}
		const completedMessage = payloads.find(
			(payload) =>
				payload.type === "message.updated" &&
				payload.properties.info.id === "a1" &&
				payload.properties.info.time.completed != null,
		)

		expect(completedMessage?.properties.info.time).toEqual({
			created: 1_772_000_000_000,
			completed: 1_772_000_000_001,
		})
	})

	test("does not parent assistant updates to a stale user after a new prompt", async () => {
		Date.now = () => 1_772_000_000_000
		const transport = new FakeTransport((method, _params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") {
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "u1",
						content: { type: "text", text: "old prompt" },
					},
				} satisfies AcpSessionNotification)
				return { configOptions }
			}
			if (method === "session/prompt") return { stopReason: "end_turn" }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		await client.session.messages({ sessionID: "s1" })
		await client.session.promptAsync({
			sessionID: "s1",
			parts: [{ type: "text", text: "new prompt without user echo" }],
		})
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "agent_message_chunk",
				messageId: "a2",
				content: { type: "text", text: "new answer" },
			},
		} satisfies AcpSessionNotification)

		const result = await client.session.messages({ sessionID: "s1" })

		expect(result.data).toEqual([
			{
				info: {
					id: "u1",
					sessionID: "s1",
					role: "user",
					time: { created: 1_772_000_000_000 },
				},
				parts: [
					{
						id: "u1-text",
						sessionID: "s1",
						messageID: "u1",
						type: "text",
						text: "old prompt",
						time: { start: 1_772_000_000_000 },
					},
				],
			},
			{
				info: {
					id: "a2",
					sessionID: "s1",
					role: "assistant",
					time: { created: 1_772_000_000_001 },
				},
				parts: [
					{
						id: "a2-text",
						sessionID: "s1",
						messageID: "a2",
						type: "text",
						text: "new answer",
						time: { start: 1_772_000_000_001 },
					},
				],
			},
		])
	})

	test("preserves file prompt parts as ACP resource links", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/prompt") return { stopReason: "end_turn" }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		await client.session.promptAsync({
			sessionID: "s1",
			parts: [
				{ type: "text", text: "read this" },
				{
					type: "file",
					url: "file:///repo/src/main.rs",
					filename: "main.rs",
					mime: "text/x-rust",
				},
			],
		})

		expect(transport.requests.at(-1)).toEqual({
			method: "session/prompt",
			directory: "/repo",
			params: {
				sessionId: "s1",
				prompt: [
					{ type: "text", text: "read this" },
					{
						type: "resource_link",
						uri: "file:///repo/src/main.rs",
						name: "main.rs",
						mimeType: "text/x-rust",
					},
				],
			},
		})
	})

	test("maps ACP reasoning effort options to model variants and applies selected effort", async () => {
		const optionsWithReasoning = [
			...configOptions,
			{
				type: "select",
				id: "thought_level",
				name: "Reasoning effort",
				category: "thought_level",
				currentValue: "medium",
				options: [
					{ value: "low", name: "Low", description: "Fast responses" },
					{ value: "medium", name: "Medium", description: "Balanced responses" },
					{ value: "high", name: "High", description: "Deeper reasoning" },
				],
			},
		] satisfies AcpSessionConfigOption[]
		const transport = new FakeTransport((method, _params, _directory, tx) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") {
				tx?.emitSessionUpdate({
					sessionId: "s1",
					update: {
						sessionUpdate: "user_message_chunk",
						messageId: "u1",
						content: { type: "text", text: "hello" },
					},
				} satisfies AcpSessionNotification)
				return { configOptions: optionsWithReasoning }
			}
			if (method === "session/set_config_option") return { configOptions: optionsWithReasoning }
			if (method === "session/prompt") return { stopReason: "end_turn" }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		await client.session.messages({ sessionID: "s1" })
		const providers = await client.config.providers()
		await client.session.promptAsync({
			sessionID: "s1",
			parts: [{ type: "text", text: "think harder" }],
			variant: "high",
		})

		expect(providers.data.providers[0].models["test-openai"]).toEqual({
			name: "Test OpenAI",
			description: "OpenAI: test-model",
			capabilities: {
				reasoning: true,
				input: { image: false, pdf: false },
				attachment: false,
			},
			variants: {
				low: { name: "Low", description: "Fast responses" },
				medium: { name: "Medium", description: "Balanced responses" },
				high: { name: "High", description: "Deeper reasoning" },
			},
			currentVariant: "medium",
			allowDefaultVariant: false,
		})
		expect(transport.requests.at(-2)).toEqual({
			method: "session/set_config_option",
			directory: "/repo",
			params: { sessionId: "s1", configId: "thought_level", value: "high" },
		})
		expect(transport.requests.at(-1)).toEqual({
			method: "session/prompt",
			directory: "/repo",
			params: {
				sessionId: "s1",
				prompt: [{ type: "text", text: "think harder" }],
			},
		})
	})

	test("maps ACP plan updates to todo events", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "plan",
				entries: [
					{ content: "Inspect", status: "completed", priority: "medium" },
					{ content: "Patch", status: "in_progress", priority: "medium" },
				],
			},
		} satisfies AcpSessionNotification)

		const first = await stream.next()
		const second = await stream.next()

		expect(first.value.payload.type).toBe("session.updated")
		expect(second.value.payload).toEqual({
			type: "todo.updated",
			properties: {
				sessionID: "s1",
				todos: [
					{ content: "Inspect", status: "completed" },
					{ content: "Patch", status: "in_progress" },
				],
			},
		})
	})

	test("maps ACP command, mode, usage, and rich tool updates", async () => {
		Date.now = () => 1_772_000_000_000
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "available_commands_update",
				availableCommands: [{ name: "compact", description: "Compact session" }],
			},
		} satisfies AcpSessionNotification)
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "current_mode_update", currentModeId: "plan" },
		} satisfies AcpSessionNotification)
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "usage_update", used: 42, size: 100 },
		} satisfies AcpSessionNotification)
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "tool_call",
				toolCallId: "tool-rich",
				title: "Patch file",
				kind: "edit",
				status: "completed",
				rawInput: {},
				content: [
					{ type: "diff", path: "src/main.rs", oldText: "old\n", newText: "new\n" },
					{ type: "content", content: { type: "text", text: "applied" } },
				],
				locations: [{ path: "src/main.rs", line: 12 }],
			},
		} satisfies AcpSessionNotification)

		expect((await nextPayload(stream, "commands-session")).type).toBe("session.updated")
		expect(await nextPayload(stream, "commands")).toEqual({
			type: "session.commands.updated",
			properties: {
				sessionID: "s1",
				commands: [{ name: "compact", description: "Compact session" }],
			},
		})
		expect((await nextPayload(stream, "mode-session")).type).toBe("session.updated")
		expect(await nextPayload(stream, "mode")).toEqual({
			type: "session.mode.updated",
			properties: { sessionID: "s1", modeID: "plan" },
		})
		expect((await nextPayload(stream, "usage-session")).type).toBe("session.updated")
		expect(await nextPayload(stream, "usage")).toEqual({
			type: "session.usage.updated",
			properties: { sessionID: "s1", used: 42, size: 100, cost: undefined },
		})
		expect((await nextPayload(stream, "tool-session")).type).toBe("session.updated")
		expect((await nextPayload(stream, "tool-message")).type).toBe("message.updated")
		expect(await nextPayload(stream, "tool-part")).toEqual({
			type: "message.part.updated",
			properties: {
				part: {
					id: "tool-tool-rich-part",
					sessionID: "s1",
					messageID: "tool-tool-rich",
					type: "tool",
					callID: "tool-rich",
					tool: "edit",
					state: {
						status: "completed",
						input: {
							filePath: "src/main.rs",
							oldString: "old\n",
							newString: "new\n",
							path: "src/main.rs",
						},
						output: "applied",
						title: "Patch file",
						metadata: {
							acpContent: [
								{ type: "diff", path: "src/main.rs", oldText: "old\n", newText: "new\n" },
								{ type: "content", content: { type: "text", text: "applied" } },
							],
							acpLocations: [{ path: "src/main.rs", line: 12 }],
						},
						time: { start: 1_772_000_000_000, end: 1_772_000_000_000 },
					},
				},
			},
		})
	})

	test("reads workspace changes through the runtime workspace API", async () => {
		const transport = new FakeTransport((method, params, directory) => {
			if (method === "workspace/changes/read") {
				expect(directory).toBe("/repo")
				expect(params).toEqual({
					sessionId: "s1",
					scopes: ["turn"],
					diffDetail: "full",
					turnId: "t1",
					maxDiffBytes: 2_000_000,
				})
				// Canonical camelCase wire shape; the client converts views back
				// to the legacy generated shape for the renderer.
				return {
					views: [
						{
							scope: workspaceChangeView.scope,
							status: workspaceChangeView.status,
							workspaceRoot: workspaceChangeView.workspace_root,
							base: {
								kind: "turn_checkpoint",
								turnId: workspaceChangeView.base.turn_id,
								checkpointId: workspaceChangeView.base.checkpoint_id,
								backend: workspaceChangeView.base.backend,
							},
							coverage: workspaceChangeView.coverage,
							attribution: workspaceChangeView.attribution,
							changeSetStatus: workspaceChangeView.change_set_status,
							files: workspaceChangeView.files,
							stats: workspaceChangeView.stats,
							unifiedDiff: workspaceChangeView.unified_diff,
							warnings: workspaceChangeView.warnings,
							generatedAt: workspaceChangeView.generated_at,
						},
					],
				}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const result = await client.workspace.changes.read({
			sessionID: "s1",
			scopes: ["turn"],
			turnID: "t1",
			diffDetail: "full",
			maxDiffBytes: 2_000_000,
		})

		expect(result.data).toEqual({ views: [workspaceChangeView] })
	})

	test("emits workspace change events from direct workspace notifications", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		transport.emitNotification("workspace/changes/updated", {
			session_id: "s1",
			turn_id: "t1",
			scope: "turn",
			status: "ready",
			coverage: "git_visible",
			change_set_status: "finalized",
			stats: { files_changed: 1, additions: 2, deletions: 1 },
			version: 42,
			generated_at: "2026-06-26T00:00:00Z",
		})

		expect(await nextPayload(stream, "workspace-direct")).toEqual({
			type: "workspace.changes.updated",
			properties: {
				sessionID: "s1",
				turnID: "t1",
				scope: "turn",
				status: "ready",
				coverage: "git_visible",
				changeSetStatus: "finalized",
				stats: { filesChanged: 1, additions: 2, deletions: 1 },
				version: 42,
				generatedAt: "2026-06-26T00:00:00Z",
			},
		})
	})

	test("emits workspace change events from wrapped original server events", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalEvent": {
					kind: "workspace_changes_updated",
					session_id: "s1",
					turn_id: "t1",
					scope: "turn",
					status: "ready",
					coverage: "git_visible",
					change_set_status: "finalized",
					stats: { files_changed: 1, additions: 2, deletions: 1 },
					version: 43,
					generated_at: "2026-06-26T00:00:01Z",
				},
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayload(stream, "workspace-session-update")).toEqual({
			type: "session.updated",
			properties: { info: expect.any(Object), session: expect.any(Object) },
		})
		expect(await nextPayload(stream, "workspace-wrapped")).toEqual({
			type: "workspace.changes.updated",
			properties: {
				sessionID: "s1",
				turnID: "t1",
				scope: "turn",
				status: "ready",
				coverage: "git_visible",
				changeSetStatus: "finalized",
				stats: { filesChanged: 1, additions: 2, deletions: 1 },
				version: 43,
				generatedAt: "2026-06-26T00:00:01Z",
			},
		})
	})

	test("maps ACP permission requests to permission events and replies", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		const permissionRequest = {
			sessionId: "s1",
			toolCall: {
				toolCallId: "tool1",
				title: "Run command",
				kind: "execute",
				rawInput: { command: "pnpm test" },
			},
			options: [
				{ optionId: "allow-once", name: "Allow once", kind: "allow_once" },
				{ optionId: "reject-once", name: "Reject", kind: "reject_once" },
			],
		} satisfies AcpRequestPermissionParams

		await client.session.list()
		transport.emitRequest(7, "session/request_permission", permissionRequest)
		const event = await stream.next()
		await client.permission.reply({ requestID: "acp-permission-7", reply: "once" })

		expect(event.value.payload).toEqual({
			type: "permission.asked",
			properties: {
				id: "acp-permission-7",
				requestID: "acp-permission-7",
				sessionID: "s1",
				permission: "Run command",
				metadata: {
					tool: "execute",
					command: "pnpm test",
				},
			},
		})
		expect(transport.responses).toEqual([
			{
				id: 7,
				result: { outcome: { outcome: "selected", optionId: "allow-once" } },
			},
		])
	})

	test("updates session title from ACP session info updates", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "session_info_update",
				title: "Renamed by server",
				updatedAt: "2026-06-24T01:00:00.000Z",
				_meta: {
					"devo/session": {
						created_at: "2026-06-24T00:00:00.000Z",
						updated_at: "2026-06-24T01:00:00.000Z",
						last_activity_at: "2026-06-24T00:00:00.000Z",
					},
				},
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayload(stream, "title")).toEqual({
			type: "session.updated",
			properties: {
				info: {
					id: "s1",
					title: "Renamed by server",
					directory: "/repo",
					parentID: undefined,
					time: {
						created: Date.parse("2026-06-24T00:00:00.000Z"),
						updated: Date.parse("2026-06-24T01:00:00.000Z"),
						lastActivity: Date.parse("2026-06-24T00:00:00.000Z"),
					},
					totalInputTokens: 0,
					totalOutputTokens: 0,
					totalTokens: 0,
					totalCacheCreationTokens: 0,
					totalCacheReadTokens: 0,
					promptTokenEstimate: 0,
					lastQueryTotalTokens: 0,
				},
				session: {
					id: "s1",
					title: "Renamed by server",
					directory: "/repo",
					parentID: undefined,
					time: {
						created: Date.parse("2026-06-24T00:00:00.000Z"),
						updated: Date.parse("2026-06-24T01:00:00.000Z"),
						lastActivity: Date.parse("2026-06-24T00:00:00.000Z"),
					},
					totalInputTokens: 0,
					totalOutputTokens: 0,
					totalTokens: 0,
					totalCacheCreationTokens: 0,
					totalCacheReadTokens: 0,
					promptTokenEstimate: 0,
					lastQueryTotalTokens: 0,
				},
			},
		})
	})

	test("does not treat session info updatedAt as last activity without Devo metadata", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "session_info_update",
				title: "Metadata-only rename",
				updatedAt: "2026-06-24T01:00:00.000Z",
			},
		} satisfies AcpSessionNotification)

		const payload = await nextPayload(stream, "metadata-only")

		expect(payload.properties.info.time).toEqual({
			created: Date.parse("2026-06-24T00:00:00.000Z"),
			updated: Date.parse("2026-06-24T01:00:00.000Z"),
			lastActivity: Date.parse("2026-06-24T00:00:00.000Z"),
		})
	})

	test("updates session last activity from ACP activity metadata", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "agent_message_chunk",
				messageId: "a-activity",
				content: { type: "text", text: "live" },
				_meta: { "devo/activityAt": "2026-06-24T00:05:00.000Z" },
			},
		} satisfies AcpSessionNotification)

		expect((await nextPayload(stream, "assistant-activity")).properties.info.time).toEqual({
			created: Date.parse("2026-06-24T00:00:00.000Z"),
			updated: Date.parse("2026-06-24T00:00:00.000Z"),
			lastActivity: Date.parse("2026-06-24T00:05:00.000Z"),
		})

		await nextPayload(stream, "assistant-message")
		await nextPayload(stream, "assistant-part")
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "tool_call_update",
				toolCallId: "tool-activity",
				status: "completed",
				rawOutput: "done",
				_meta: { "devo/activityAt": "2026-06-24T00:06:00.000Z" },
			},
		} satisfies AcpSessionNotification)

		expect((await nextPayload(stream, "tool-activity")).properties.info.time).toEqual({
			created: Date.parse("2026-06-24T00:00:00.000Z"),
			updated: Date.parse("2026-06-24T00:00:00.000Z"),
			lastActivity: Date.parse("2026-06-24T00:06:00.000Z"),
		})
	})

	test("keeps session update time stable when replaying loaded history", async () => {
		Date.now = () => Date.parse("2026-06-24T02:00:00.000Z")
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "user_message_chunk",
				messageId: "u1",
				content: { type: "text", text: "hello" },
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayload(stream, "history replay")).toEqual({
			type: "session.updated",
			properties: {
				info: {
					id: "s1",
					title: "Existing session",
					directory: "/repo",
					parentID: undefined,
					time: {
						created: Date.parse("2026-06-24T00:00:00.000Z"),
						updated: Date.parse("2026-06-24T00:00:00.000Z"),
						lastActivity: Date.parse("2026-06-24T00:00:00.000Z"),
					},
					totalInputTokens: 0,
					totalOutputTokens: 0,
					totalTokens: 0,
					totalCacheCreationTokens: 0,
					totalCacheReadTokens: 0,
					promptTokenEstimate: 0,
					lastQueryTotalTokens: 0,
				},
				session: {
					id: "s1",
					title: "Existing session",
					directory: "/repo",
					parentID: undefined,
					time: {
						created: Date.parse("2026-06-24T00:00:00.000Z"),
						updated: Date.parse("2026-06-24T00:00:00.000Z"),
						lastActivity: Date.parse("2026-06-24T00:00:00.000Z"),
					},
					totalInputTokens: 0,
					totalOutputTokens: 0,
					totalTokens: 0,
					totalCacheCreationTokens: 0,
					totalCacheReadTokens: 0,
					promptTokenEstimate: 0,
					lastQueryTotalTokens: 0,
				},
			},
		})
	})

	test("uses canonical session/metadata/update for title updates and emits deletion events", async () => {
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/metadata/update") {
				// Canonical Session (camelCase) result; the client rebuilds the
				// legacy devo/session metadata for session state folding.
				return {
					session: {
						id: (params as { sessionId: string }).sessionId,
						cwd: "/repo",
						createdAt: "2026-06-24T01:00:00.000Z",
						lastActivityAt: "2026-06-24T01:00:00.000Z",
						title: "New title",
						ephemeral: false,
						status: "idle",
						usage: {
							total: {
								totalTokens: 0,
								inputTokens: 0,
								outputTokens: 0,
								cacheCreationInputTokens: 0,
								cacheReadInputTokens: 0,
							},
						},
					},
				}
			}
			if (method === "session/delete") return {}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		await client.session.update({ sessionID: "s1", title: "New title" })
		await client.session.delete({ sessionID: "s1" })

		expect(transport.requests.at(-2)).toEqual({
			method: "session/metadata/update",
			directory: "/repo",
			params: { sessionId: "s1", expectedVersion: 0, title: "New title" },
		})
		expect(await nextPayload(stream, "renamed")).toEqual({
			type: "session.updated",
			properties: {
				info: {
					id: "s1",
					title: "New title",
					directory: "/repo",
					parentID: undefined,
					time: {
						created: Date.parse("2026-06-24T01:00:00.000Z"),
						updated: Date.parse("2026-06-24T01:00:00.000Z"),
						lastActivity: Date.parse("2026-06-24T01:00:00.000Z"),
					},
					totalInputTokens: 0,
					totalOutputTokens: 0,
					totalTokens: 0,
					totalCacheCreationTokens: 0,
					totalCacheReadTokens: 0,
					promptTokenEstimate: 0,
					lastQueryTotalTokens: 0,
				},
				session: {
					id: "s1",
					title: "New title",
					directory: "/repo",
					parentID: undefined,
					time: {
						created: Date.parse("2026-06-24T01:00:00.000Z"),
						updated: Date.parse("2026-06-24T01:00:00.000Z"),
						lastActivity: Date.parse("2026-06-24T01:00:00.000Z"),
					},
					totalInputTokens: 0,
					totalOutputTokens: 0,
					totalTokens: 0,
					totalCacheCreationTokens: 0,
					totalCacheReadTokens: 0,
					promptTokenEstimate: 0,
					lastQueryTotalTokens: 0,
				},
			},
		})
		expect(await nextPayload(stream, "deleted")).toEqual({
			type: "session.deleted",
			properties: {
				info: {
					id: "s1",
					directory: "/repo",
				},
			},
		})
	})

	test("emits local deletion events when a server broadcast reports deleted session ids", async () => {
		const childSessionInfo = {
			...sessionInfo,
			sessionId: "s2",
			title: "Forked session",
		} satisfies AcpSessionInfo
		let deleted = false
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") {
				return { sessions: deleted ? [] : [sessionInfo, childSessionInfo] }
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "session/deleted",
				"devo/originalEvent": {
					kind: "session_deleted",
					session_id: "s1",
					deleted_session_ids: ["s1", "s2"],
				},
			},
		} as AcpSessionNotification)
		deleted = true

		expect(await nextPayloadOfType(stream, "session.deleted", "root deleted")).toEqual({
			type: "session.deleted",
			properties: {
				info: {
					id: "s1",
					directory: "/repo",
				},
			},
		})
		expect(await nextPayloadOfType(stream, "session.deleted", "child deleted")).toEqual({
			type: "session.deleted",
			properties: {
				info: {
					id: "s2",
					directory: "/repo",
				},
			},
		})
		expect((await client.session.get({ sessionID: "s1" })).data).toBeUndefined()
		expect((await client.session.get({ sessionID: "s2" })).data).toBeUndefined()
	})

	test("follows ACP session list pagination until caller limit is satisfied", async () => {
		const pages = new Map<string | undefined, AcpSessionInfo[]>([
			[undefined, [sessionInfo]],
			[
				"page-2",
				[
					{
						sessionId: "s2",
						cwd: "/repo",
						title: "Second session",
						updatedAt: "2026-06-24T00:01:00.000Z",
					},
				],
			],
		])
		const transport = new FakeTransport((method, params) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") {
				const cursor = (params as { cursor?: string }).cursor
				return {
					sessions: pages.get(cursor) ?? [],
					nextCursor: cursor ? null : "page-2",
				}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		const result = await client.session.list({ limit: 2 })

		expect(result.data.map((session) => session.id)).toEqual(["s1", "s2"])
		expect(transport.requests.filter((request) => request.method === "session/list")).toEqual([
			{ method: "session/list", directory: "/repo", params: { cwd: "/repo" } },
			{ method: "session/list", directory: "/repo", params: { cwd: "/repo", cursor: "page-2" } },
		])
	})
	test("maps active_turn session list metadata to busy session status", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") {
				return {
					sessions: [
						{
							...sessionInfo,
							_meta: { "devo/session": { status: "active_turn" } },
						},
					],
				}
			}
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()

		expect(await client.session.status()).toEqual({ data: { s1: { type: "busy" } } })
	})

	test("emits session status updates from wrapped original SessionStatusChanged events", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "session/status/changed",
				"devo/originalEvent": {
					SessionStatusChanged: {
						session_id: "s1",
						status: "active_turn",
					},
				},
			},
		} satisfies AcpSessionNotification)
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "session/status/changed",
				"devo/originalEvent": {
					session_id: "s1",
					status: "idle",
				},
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayloadOfType(stream, "session.status", "wrapped busy")).toEqual({
			type: "session.status",
			properties: {
				sessionID: "s1",
				status: { type: "busy" },
			},
		})
		expect(await nextPayloadOfType(stream, "session.status", "wrapped idle")).toEqual({
			type: "session.status",
			properties: {
				sessionID: "s1",
				status: { type: "idle" },
			},
		})
		expect(await client.session.status()).toEqual({ data: { s1: { type: "idle" } } })
	})

	test("emits compaction lifecycle events from wrapped original events with nested session metadata", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "session/compaction/started",
				"devo/originalEvent": {
					kind: "session_compaction_started",
					session: {
						session_id: "s1",
						title: "Existing session",
					},
				},
			},
		} satisfies AcpSessionNotification)
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "session/compaction/completed",
				"devo/originalEvent": {
					SessionCompactionCompleted: {
						session: {
							sessionId: "s1",
							title: "Existing session",
						},
					},
				},
			},
		} satisfies AcpSessionNotification)
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "session/compaction/failed",
				"devo/originalEvent": {
					SessionCompactionFailed: {
						session_id: "s1",
						message: "provider unavailable",
					},
				},
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayloadOfType(stream, "session.compaction.started", "started")).toEqual({
			type: "session.compaction.started",
			properties: { sessionID: "s1" },
		})
		expect(await nextPayloadOfType(stream, "session.compaction.completed", "completed")).toEqual({
			type: "session.compaction.completed",
			properties: { sessionID: "s1" },
		})
		expect(await nextPayloadOfType(stream, "session.compaction.failed", "failed")).toEqual({
			type: "session.compaction.failed",
			properties: { sessionID: "s1", message: "provider unavailable" },
		})
	})

	test("bridges turn context-compaction items into lifecycle events and a transcript marker", async () => {
		Date.now = () => 1_772_000_000_000
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		const context = {
			session_id: "s1",
			turn_id: "turn-1",
			item_id: "compact-1",
			seq: 9,
		}
		const item = {
			item_id: "compact-1",
			item_kind: "context_compaction",
			payload: { title: "Compaction started" },
		}
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "item/started",
				"devo/originalEvent": { kind: "item_started", context, item },
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayloadOfType(stream, "session.compaction.started", "item started")).toEqual({
			type: "session.compaction.started",
			properties: { sessionID: "s1" },
		})
		expect(await nextPayloadOfType(stream, "message.part.updated", "compaction marker")).toEqual({
			type: "message.part.updated",
			properties: {
				part: {
					id: "compaction-compact-1-text",
					sessionID: "s1",
					messageID: "compaction-compact-1",
					type: "text",
					text: "Session compaction started.",
					time: { start: 1_772_000_000_000 },
				},
			},
		})

		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "item/completed",
				"devo/originalEvent": {
					kind: "item_completed",
					context,
					item: { ...item, payload: { title: "Context compacted" } },
				},
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayloadOfType(stream, "session.compaction.completed", "item completed")).toEqual({
			type: "session.compaction.completed",
			properties: { sessionID: "s1" },
		})

		transport.emitSessionUpdate({
			sessionId: "s1",
			update: { sessionUpdate: "session_info_update" },
			_meta: {
				"devo/originalMethod": "item/completed",
				"devo/originalEvent": {
					kind: "item_completed",
					context: { ...context, item_id: "compact-2" },
					item: {
						...item,
						item_id: "compact-2",
						payload: {
							title: "Compaction failed",
							status: "failed",
							message: "summary unavailable",
						},
					},
				},
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayloadOfType(stream, "session.compaction.failed", "item failed")).toEqual({
			type: "session.compaction.failed",
			properties: { sessionID: "s1", message: "summary unavailable" },
		})
	})

	test("refreshes session status from session info update metadata", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.list()
		transport.emitSessionUpdate({
			sessionId: "s1",
			update: {
				sessionUpdate: "session_info_update",
				_meta: { "devo/session.status": "active_turn" },
			},
		} satisfies AcpSessionNotification)

		expect(await nextPayloadOfType(stream, "session.status", "metadata busy")).toEqual({
			type: "session.status",
			properties: {
				sessionID: "s1",
				status: { type: "busy" },
			},
		})
		expect(await client.session.status()).toEqual({ data: { s1: { type: "busy" } } })
	})

	test("loading session history keeps status idle without runtime metadata or events", async () => {
		const transport = new FakeTransport((method) => {
			if (method === "initialize") return initializeResult
			if (method === "session/list") return { sessions: [sessionInfo] }
			if (method === "session/load") return { configOptions }
			throw new Error(`unexpected request ${method}`)
		})
		const client = createDevoClient({ directory: "/repo", transport })

		await client.session.list()
		await client.session.messages({ sessionID: "s1" })

		expect(await client.session.status()).toEqual({ data: { s1: { type: "idle" } } })
	})

})
