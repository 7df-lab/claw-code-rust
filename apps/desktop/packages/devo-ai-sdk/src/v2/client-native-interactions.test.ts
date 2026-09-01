import { describe, expect, test } from "bun:test"
import {
	DESKTOP_INITIALIZE_PARAMS,
	createDevoClient,
	type DevoNativeTransport,
	type DevoNativeTransportEvent,
} from "./client"

class FakeNativeTransport implements DevoNativeTransport {
	readonly requests: Array<{ method: string; params: unknown; directory?: string }> = []
	readonly responses: Array<{ id: string | number; result: unknown }> = []
	private listeners: Array<(event: DevoNativeTransportEvent) => void> = []
	subscriptionCreateHook?: () => void
	pendingControlRequests: unknown[] = []
	subscriptionCursors: Array<{ streamId: string; seq: number }> = []
	sessionItems: unknown[] = []
	resumeSession?: unknown

	async request(method: string, params?: unknown, directory?: string): Promise<unknown> {
		this.requests.push({ method, params, directory })
		switch (method) {
			case "initialize":
				return { protocolVersion: 1, agentCapabilities: {}, authMethods: [] }
			case "session/new":
				return { session: nativeSession }
			case "session/list":
				return { data: [nativeSession], nextCursor: null }
			case "subscription/create":
				this.subscriptionCreateHook?.()
				return {
					subscriptionId: `sub-${this.requests.length}`,
					cursors: this.subscriptionCursors,
					pendingControlRequests: this.pendingControlRequests,
				}
			case "subscription/ack":
				return { serverTimeMs: 1 }
			case "skill/list":
				return { skills: [nativeSkill] }
			case "skill/set_enabled":
				return { skills: [{ ...nativeSkill, enabled: false }] }
			case "mcp/list":
				return { servers: [{ name: "docs", status: "connected", toolCount: 2 }] }
			case "mcp/tools":
				return {
					tools: [{ name: "get_time", description: "Current time" }],
				}
			case "workspace/changes/read":
				return { views: [nativeWorkspaceView] }
			case "turn/start":
				return { turn: nativeTurnInProgress }
			case "session/message/edit":
				return editedMessageResult
			case "session/resume":
				return {
					session: this.resumeSession ?? nativeSession,
					lastContextOccupancy: nativeOccupancy,
				}
			case "session/items/list":
				return { data: this.sessionItems, nextCursor: null }
			case "context/usage/read":
				return { occupancy: nativeOccupancy }
			default:
				throw new Error(`unexpected request ${method}`)
		}
	}

	async respond(id: string | number, result: unknown): Promise<void> {
		this.responses.push({ id, result })
	}

	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void {
		this.listeners.push(listener)
		return () => {
			this.listeners = this.listeners.filter((candidate) => candidate !== listener)
		}
	}

	connected(): boolean {
		return true
	}

	emit(event: DevoNativeTransportEvent): void {
		for (const listener of this.listeners) listener(event)
	}
}

const nativeSkill = {
	id: "review",
	name: "review",
	description: "Review code",
	path: "/skills/review",
	enabled: true,
	source: "User",
	scope: "user",
}

const nativeWorkspaceView = {
	scope: "uncommitted",
	status: "ready",
	workspaceRoot: "/repo",
	coverage: "git_visible",
	attribution: "git_working_tree",
	changeSetStatus: "finalized",
	files: [],
	stats: { files_changed: 0, additions: 0, deletions: 0 },
	unifiedDiff: "diff --git a/a b/a\n",
	warnings: [],
	generatedAt: "2026-08-22T00:00:00Z",
}

const nativeSession = {
	id: "session-1",
	version: 1,
	cwd: "/repo",
	title: "Native session",
	parent: null,
	createdAt: "2026-08-22T00:00:00Z",
	lastActivityAt: "2026-08-22T00:00:00Z",
	status: "idle",
	flags: [],
	archived: false,
	ephemeral: false,
	model: { provider: "test", model: "test-model" },
	settings: { permissionProfile: "default" },
	preview: "",
	queuedCount: 0,
	usage: {
		total: {
			inputTokens: 0,
			outputTokens: 0,
			cacheCreationInputTokens: 0,
			cacheReadInputTokens: 0,
			reasoningTokens: 0,
			totalTokens: 0,
			callCount: 0,
			meteredCallCount: 0,
			failedCallCount: 0,
			cancelledCallCount: 0,
		},
		byPurpose: [],
		updatedAt: "2026-08-22T00:00:00Z",
	},
}

const nativeTurnInProgress = {
	id: "turn-1",
	sessionId: nativeSession.id,
	sequence: 1,
	kind: "regular",
	status: "inProgress",
	model: nativeSession.model,
	startedAt: "2026-08-24T00:00:00Z",
}

const nativeTurnCompleted = {
	...nativeTurnInProgress,
	status: "completed",
	completedAt: "2026-08-24T00:00:08Z",
}

const editedMessageResult = {
	editState: "accepted",
	replacementTurnId: "turn-2",
	item: {
		id: "item-user-edited",
		sessionId: nativeSession.id,
		turnId: "turn-2",
		revision: 2,
		seq: 1,
		state: "completed",
		createdAt: "2026-08-24T00:00:10.000Z",
		updatedAt: "2026-08-24T00:00:10.000Z",
		item: {
			type: "userMessage",
			content: [{ type: "text", text: "edited" }],
			entry: "turnStart",
		},
	},
}

const nativeOccupancy = {
	totalTokens: 100_000,
	contextWindowTokens: 200_000,
	categories: [
		{ id: "base", tokens: 10_000, shareBps: 1000 },
		{ id: "skills", tokens: 5_000, shareBps: 500 },
		{ id: "toolsBuiltin", tokens: 20_000, shareBps: 2000 },
		{ id: "toolsMcp", tokens: 15_000, shareBps: 1500 },
		{ id: "conversation", tokens: 50_000, shareBps: 5000 },
	],
}

const approvalItem = {
	type: "approval",
	approvalId: "approval-1",
	actionSummary: "Run cargo test",
	justification: "Verify the change",
	resource: "process",
	availableScopes: ["once", "session", "commandPrefixPersist"],
	commandPattern: ["cargo", "test", "*"],
	commandPrefix: ["cargo", "test"],
	target: { kind: "command", command: "cargo test -p devo-server" },
}

const approvalEnvelope = {
	id: "item-approval-1",
	sessionId: "session-1",
	turnId: "turn-1",
	revision: 1,
	seq: 1,
	state: "waiting",
	createdAt: "2026-08-22T00:00:01Z",
	updatedAt: "2026-08-22T00:00:01Z",
	item: approvalItem,
}

const userInputItem = {
	type: "userInputRequest",
	requestId: "input-1",
	questions: [
		{
			id: "environment",
			header: "Environment",
			question: "Where should this run?",
			isOther: false,
			isSecret: false,
			options: [{ label: "Local", description: "Use this machine" }],
		},
	],
}

const userInputEnvelope = {
	id: "item-input-1",
	sessionId: "session-1",
	turnId: "turn-1",
	revision: 1,
	seq: 1,
	state: "waiting",
	createdAt: "2026-08-22T00:00:02Z",
	updatedAt: "2026-08-22T00:00:02Z",
	item: userInputItem,
}

async function nextPayloadOfType(stream: AsyncIterator<any>, type: string): Promise<any> {
	const deadline = Date.now() + 1_000
	while (Date.now() < deadline) {
		const result = await Promise.race([
			stream.next(),
			new Promise<IteratorResult<any>>((resolve) =>
				setTimeout(() => resolve({ done: false, value: { payload: { type: "timeout" } } }), 25),
			),
		])
		if (result.value?.payload?.type === type) return result.value.payload
	}
	throw new Error(`timed out waiting for ${type}`)
}

describe("Native desktop SDK interactions", () => {
	test("global event consumers subscribe to every existing Native session", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })

		await client.event.subscribe()

		expect(transport.requests).toEqual([
			{ method: "initialize", params: DESKTOP_INITIALIZE_PARAMS, directory: "/repo" },
			{
				method: "session/list",
				params: { cwds: ["/repo"] },
				directory: "/repo",
			},
			{
				method: "subscription/create",
				params: {
					selectors: [{ kind: "session", sessionId: "session-1" }],
					includeSnapshot: true,
					after: [],
				},
				directory: "/repo",
			},
		])
	})

	test("initializes and uses Native session RPC wire shapes", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })

		const created = await client.session.create()
		const listed = await client.session.list({ limit: 10 })

		expect(created.data.id).toBe("session-1")
		expect(listed.data.map((session: any) => session.id)).toEqual(["session-1"])
		expect(transport.requests).toEqual([
			{ method: "initialize", params: DESKTOP_INITIALIZE_PARAMS, directory: "/repo" },
			{
				method: "session/new",
				params: { cwd: "/repo", idempotencyKey: expect.any(String) },
				directory: "/repo",
			},
			{
				method: "subscription/create",
				params: {
					selectors: [{ kind: "session", sessionId: "session-1" }],
					includeSnapshot: true,
					after: [],
				},
				directory: "/repo",
			},
			{
				method: "session/list",
				params: { cwds: ["/repo"], limit: 10 },
				directory: "/repo",
			},
		])
	})

	test("uses Native skill, MCP, and workspace diff RPCs", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })

		expect((await client.app.skills()).data).toEqual([nativeSkill])
		expect((await client.mcp.list()).data).toEqual([
			{ name: "docs", status: "connected", toolCount: 2 },
		])
		expect((await client.mcp.tools({ name: "docs" })).data).toEqual([
			{ name: "get_time", description: "Current time" },
		])
		expect((await client.app.setSkillEnabled({ path: "/skills/review", enabled: false })).data).toEqual([
			{ ...nativeSkill, enabled: false },
		])
		expect((await client.session.diff({ sessionID: "session-1" })).data).toEqual([
			{ diff: "diff --git a/a b/a\n" },
		])
		expect(transport.requests.slice(1)).toEqual([
			{
				method: "skill/list",
				params: { cwd: "/repo", forceReload: false },
				directory: "/repo",
			},
			{ method: "mcp/list", params: {}, directory: "/repo" },
			{ method: "mcp/tools", params: { name: "docs" }, directory: "/repo" },
			{
				method: "skill/set_enabled",
				params: { path: "/skills/review", enabled: false, cwd: "/repo" },
				directory: "/repo",
			},
			{
				method: "workspace/changes/read",
				params: {
					sessionId: "session-1",
					scopes: ["uncommitted"],
					diffDetail: "full",
					maxDiffBytes: 2_000_000,
				},
				directory: "/repo",
			},
		])
	})

	test("registers approval before the item event and responds to its JSON-RPC id", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		transport.emit({
			type: "request",
			id: 41,
			method: "approval/command/request",
			params: approvalItem,
		})
		transport.emit({
			type: "notification",
			method: "item/started",
			params: { item: approvalEnvelope },
		})

		const asked = await nextPayloadOfType(stream, "permission.asked")
		expect(asked.properties).toEqual({
			id: "approval-1",
			requestID: "approval-1",
			sessionID: "session-1",
			permission: "Run cargo test",
			metadata: {
				tool: "process",
				command: "cargo test -p devo-server",
				path: undefined,
				host: undefined,
				justification: "Verify the change",
				resource: "process",
				target: "cargo test -p devo-server",
				availableScopes: ["once", "session", "commandPrefixPersist"],
				commandPattern: ["cargo", "test", "*"],
				commandPrefix: ["cargo", "test"],
			},
		})

		await client.permission.reply({
			requestID: "approval-1",
			reply: "commandPrefixPersist",
		})
		expect(transport.responses).toEqual([
			{
				id: 41,
				result: {
					requestId: "approval-1",
					decision: {
						decision: "approved",
						scope: "commandPrefixPersist",
						decisionSource: "user",
						decidedAt: expect.any(String),
					},
				},
			},
		])
	})

	test("registers user input before the item event and returns matching answers", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		transport.emit({ type: "request", id: "rpc-input", method: "userInput/request", params: userInputItem })
		transport.emit({
			type: "notification",
			method: "item/started",
			params: { item: userInputEnvelope },
		})

		const asked = await nextPayloadOfType(stream, "question.asked")
		expect(asked.properties).toEqual({
			id: "input-1",
			requestID: "input-1",
			sessionID: "session-1",
			questions: [
				{
					id: "environment",
					header: "Environment",
					question: "Where should this run?",
					isOther: false,
					isSecret: false,
					options: [{ label: "Local", description: "Use this machine" }],
				},
			],
		})

		await client.question.reply({ requestID: "input-1", answers: [["Local"]] })
		expect(transport.responses).toEqual([
			{
				id: "rpc-input",
				result: {
					requestId: "input-1",
					answers: { environment: { answers: ["Local"] } },
				},
			},
		])
	})

	test("closes user input when another controller completes its item", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		transport.emit({ type: "request", id: 51, method: "userInput/request", params: userInputItem })
		transport.emit({ type: "notification", method: "item/started", params: { item: userInputEnvelope } })
		await nextPayloadOfType(stream, "question.asked")
		transport.emit({
			type: "notification",
			method: "item/completed",
			params: {
				item: {
					...userInputEnvelope,
					revision: 2,
					state: "completed",
					item: { ...userInputItem, answers: { environment: { answers: ["Local"] } } },
				},
			},
		})

		expect((await nextPayloadOfType(stream, "question.replied")).properties).toEqual({
			sessionID: "session-1",
			requestID: "input-1",
		})
		await client.question.reply({ requestID: "input-1", answers: [["Local"]] })
		expect(transport.responses).toEqual([])
	})

	test("restores an answerable pending approval during subscription creation", async () => {
		const transport = new FakeNativeTransport()
		transport.pendingControlRequests = [
			{ requestId: "approval-1", kind: "approvalCommand", item: approvalEnvelope },
		]
		transport.subscriptionCreateHook = () => {
			transport.emit({
				type: "request",
				id: "reissued-approval",
				method: "approval/command/request",
				params: approvalItem,
			})
		}
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.create()
		const asked = await nextPayloadOfType(stream, "permission.asked")
		expect(asked.properties.requestID).toBe("approval-1")
		await client.permission.reply({ requestID: "approval-1", reply: "once" })
		expect(transport.responses[0]?.id).toBe("reissued-approval")
	})

	test("restores a waiting user-input item without a prior reverse RPC", async () => {
		const transport = new FakeNativeTransport()
		transport.pendingControlRequests = [
			{ requestId: "input-1", kind: "userInput", item: userInputEnvelope },
		]
		transport.subscriptionCreateHook = () => {
			transport.emit({
				type: "request",
				id: "reissued-input",
				method: "userInput/request",
				params: userInputItem,
			})
		}
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.create()
		const asked = await nextPayloadOfType(stream, "question.asked")
		expect(asked.properties).toEqual({
			id: "input-1",
			requestID: "input-1",
			sessionID: "session-1",
			questions: [
				{
					id: "environment",
					header: "Environment",
					question: "Where should this run?",
					isOther: false,
					isSecret: false,
					options: [{ label: "Local", description: "Use this machine" }],
				},
			],
		})
		await client.question.reply({ requestID: "input-1", answers: [["Local"]] })
		expect(transport.responses[0]?.id).toBe("reissued-input")
	})

	test("restores a waiting user-input item from session history after restart", async () => {
		const transport = new FakeNativeTransport()
		transport.sessionItems = [userInputEnvelope]
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		await client.session.messages({ sessionID: "session-1" })
		const asked = await nextPayloadOfType(stream, "question.asked")
		expect(asked.properties.requestID).toBe("input-1")
		expect(asked.properties.sessionID).toBe("session-1")
	})

	test("disconnect clears stale interactions and creates a fresh event stream", async () => {
		const transport = new FakeNativeTransport()
		transport.subscriptionCursors = [{ streamId: "session:session-1", seq: 7 }]
		const client = createDevoClient({ directory: "/repo", transport })
		const oldStream = (await client.global.event()).stream[Symbol.asyncIterator]()
		transport.emit({ type: "request", id: 9, method: "approval/permission/request", params: approvalItem })

		transport.emit({ type: "closed", error: "transport stopped" })
		expect((await oldStream.next()).done).toBe(true)
		await client.permission.reply({ requestID: "approval-1", reply: "once" })
		expect(transport.responses).toEqual([])

		const newStream = (await client.global.event()).stream[Symbol.asyncIterator]()
		expect(newStream).not.toBe(oldStream)
		expect(transport.requests.filter((request) => request.method === "initialize")).toHaveLength(2)
		expect(
			transport.requests.filter((request) => request.method === "subscription/create").at(-1)?.params,
		).toEqual({
			selectors: [{ kind: "session", sessionId: "session-1" }],
			includeSnapshot: true,
			after: [{ streamId: "session:session-1", seq: 7 }],
		})
	})

	test("does not duplicate user message text across item started and completed", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		const text = "让我们聊会天，随便聊会。"
		const envelope = {
			id: "item-user-1",
			sessionId: "session-1",
			turnId: "turn-1",
			revision: 1,
			seq: 1,
			state: "running",
			createdAt: "2026-08-22T00:00:03Z",
			updatedAt: "2026-08-22T00:00:03Z",
			item: {
				type: "userMessage",
				content: [{ type: "text", text }],
				entry: "turnStart",
			},
		}

		transport.emit({
			type: "notification",
			method: "item/started",
			params: { item: envelope },
		})
		transport.emit({
			type: "notification",
			method: "item/completed",
			params: { item: { ...envelope, revision: 2, state: "completed" } },
		})

		const parts: string[] = []
		const deadline = Date.now() + 1_000
		while (Date.now() < deadline) {
			const result = await Promise.race([
				stream.next(),
				new Promise<IteratorResult<any>>((resolve) =>
					setTimeout(() => resolve({ done: false, value: { payload: { type: "timeout" } } }), 25),
				),
			])
			if (result.value?.payload?.type === "message.part.updated") {
				const partText = result.value.payload.properties.part.text
				if (typeof partText === "string") parts.push(partText)
			}
			if (result.value?.payload?.type === "timeout" && parts.length > 0) break
		}

		expect(parts.at(-1)).toBe(text)
		expect(parts.some((part) => part === `${text}${text}`)).toBe(false)
	})

	test("completes a write tool when the matching FileChange item finishes", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		const callId = "write-1"
		transport.emit({
			type: "notification",
			method: "item/started",
			params: {
				item: {
					id: "item-write-call",
					sessionId: "session-1",
					turnId: "turn-1",
					revision: 1,
					seq: 2,
					state: "running",
					item: {
						type: "toolCall",
						callId,
						toolName: "write",
						source: "builtin",
						input: { path: "C:/Users/lenovo/Desktop/from-devo.txt", content: "hi" },
					},
				},
			},
		})
		transport.emit({
			type: "notification",
			method: "item/completed",
			params: {
				item: {
					id: "item-write-change",
					sessionId: "session-1",
					turnId: "turn-1",
					revision: 1,
					seq: 3,
					state: "completed",
					item: {
						type: "fileChange",
						callId,
						changes: [
							{
								path: "C:/Users/lenovo/Desktop/from-devo.txt",
								change: { type: "add", content: "hi" },
							},
						],
					},
				},
			},
		})

		let status = ""
		const deadline = Date.now() + 1_000
		while (Date.now() < deadline) {
			const result = await Promise.race([
				stream.next(),
				new Promise<IteratorResult<any>>((resolve) =>
					setTimeout(() => resolve({ done: false, value: { payload: { type: "timeout" } } }), 25),
				),
			])
			const payload = result.value?.payload
			if (payload?.type === "message.part.updated") {
				const part = payload.properties.part
				if (part?.callID === callId || part?.tool === "write") {
					status = part.state?.status ?? ""
					if (status === "completed") break
				}
			}
			if (payload?.type === "timeout" && status) break
		}

		expect(status).toBe("completed")
	})

	test("maps a FileChange Update completion to edit with unifiedDiff", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		const callId = "edit-update-1"
		const unifiedDiff = [
			"diff --git a/src/lib.rs b/src/lib.rs",
			"--- a/src/lib.rs",
			"+++ b/src/lib.rs",
			"@@ -1 +1,2 @@",
			" keep",
			"+added",
		].join("\n")

		transport.emit({
			type: "notification",
			method: "item/completed",
			params: {
				item: {
					id: "item-edit-change",
					sessionId: "session-1",
					turnId: "turn-1",
					revision: 1,
					seq: 2,
					state: "completed",
					item: {
						type: "fileChange",
						callId,
						changes: [
							{
								path: "/repo/src/lib.rs",
								change: { type: "update", unifiedDiff },
							},
						],
					},
				},
			},
		})

		let part: any
		const deadline = Date.now() + 1_000
		while (Date.now() < deadline) {
			const result = await Promise.race([
				stream.next(),
				new Promise<IteratorResult<any>>((resolve) =>
					setTimeout(() => resolve({ done: false, value: { payload: { type: "timeout" } } }), 25),
				),
			])
			const payload = result.value?.payload
			if (payload?.type === "message.part.updated") {
				const next = payload.properties.part
				if (next?.callID === callId) {
					part = next
					if (next.state?.status === "completed") break
				}
			}
			if (payload?.type === "timeout" && part) break
		}

		expect({
			tool: part?.tool,
			status: part?.state?.status,
			path: part?.state?.input?.path,
			changeType: part?.state?.input?.changeType,
			unifiedDiff: part?.state?.input?.unifiedDiff,
		}).toEqual({
			tool: "edit",
			status: "completed",
			path: "/repo/src/lib.rs",
			changeType: "update",
			unifiedDiff,
		})
	})

	test("preserves edit oldString/newString when FileChange Update completes", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		const callId = "edit-merge-1"
		const unifiedDiff = "@@\n-old\n+new\n"

		transport.emit({
			type: "notification",
			method: "item/started",
			params: {
				item: {
					id: "item-edit-call",
					sessionId: "session-1",
					turnId: "turn-1",
					revision: 1,
					seq: 2,
					state: "running",
					item: {
						type: "toolCall",
						callId,
						toolName: "edit",
						source: "builtin",
						input: {
							path: "/repo/a.ts",
							oldString: "old",
							newString: "new",
						},
					},
				},
			},
		})
		transport.emit({
			type: "notification",
			method: "item/completed",
			params: {
				item: {
					id: "item-edit-change",
					sessionId: "session-1",
					turnId: "turn-1",
					revision: 1,
					seq: 3,
					state: "completed",
					item: {
						type: "fileChange",
						callId,
						changes: [
							{
								path: "/repo/a.ts",
								change: { type: "update", unifiedDiff },
							},
						],
					},
				},
			},
		})

		let part: any
		const deadline = Date.now() + 1_000
		while (Date.now() < deadline) {
			const result = await Promise.race([
				stream.next(),
				new Promise<IteratorResult<any>>((resolve) =>
					setTimeout(() => resolve({ done: false, value: { payload: { type: "timeout" } } }), 25),
				),
			])
			const payload = result.value?.payload
			if (payload?.type === "message.part.updated") {
				const next = payload.properties.part
				if (next?.callID === callId) {
					part = next
					if (next.state?.status === "completed") break
				}
			}
			if (payload?.type === "timeout" && part?.state?.status === "completed") break
		}

		expect({
			tool: part?.tool,
			oldString: part?.state?.input?.oldString,
			newString: part?.state?.input?.newString,
			unifiedDiff: part?.state?.input?.unifiedDiff,
			changeType: part?.state?.input?.changeType,
		}).toEqual({
			tool: "edit",
			oldString: "old",
			newString: "new",
			unifiedDiff,
			changeType: "update",
		})
	})

	test("projects history ToolResult files/diff metadata into an edit tool part", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		const callId = "history-edit-1"
		const unifiedDiff = [
			"diff --git a/hello.py b/hello.py",
			"--- a/hello.py",
			"+++ b/hello.py",
			"@@ -1 +1 @@",
			"-old",
			"+new",
		].join("\n")

		transport.emit({
			type: "notification",
			method: "item/completed",
			params: {
				item: {
					id: "item-history-edit",
					sessionId: "session-1",
					turnId: "turn-1",
					revision: 1,
					seq: 2,
					state: "completed",
					item: {
						type: "toolResult",
						callId,
						isError: false,
						truncated: false,
						output: {
							diff: unifiedDiff,
							files: [
								{
									filePath: "C:/Users/lenovo/Desktop/hello.py",
									kind: "update",
									additions: 1,
									deletions: 1,
									oldContent: "old\n",
									preContent: "old\n",
									postContent: "new\n",
									content: "new\n",
								},
							],
						},
					},
				},
			},
		})

		let part: any
		const deadline = Date.now() + 1_000
		while (Date.now() < deadline) {
			const result = await Promise.race([
				stream.next(),
				new Promise<IteratorResult<any>>((resolve) =>
					setTimeout(() => resolve({ done: false, value: { payload: { type: "timeout" } } }), 25),
				),
			])
			const payload = result.value?.payload
			if (payload?.type === "message.part.updated") {
				const next = payload.properties.part
				if (next?.callID === callId) {
					part = next
					if (next.state?.status === "completed") break
				}
			}
			if (payload?.type === "timeout" && part) break
		}

		expect({
			tool: part?.tool,
			path: part?.state?.input?.path,
			changeType: part?.state?.input?.changeType,
			oldString: part?.state?.input?.oldString,
			newString: part?.state?.input?.newString,
			unifiedDiff: part?.state?.input?.unifiedDiff,
		}).toEqual({
			tool: "edit",
			path: "C:/Users/lenovo/Desktop/hello.py",
			changeType: "update",
			oldString: "old\n",
			newString: "new\n",
			unifiedDiff,
		})
	})

	test("uses displayContent for read ToolResult instead of stringifying Mixed output", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		const callId = "read-1"
		const displayContent = "<path>hello.py</path>\n<content>\n1: def main():\n2:    pass\n</content>"

		transport.emit({
			type: "notification",
			method: "item/completed",
			params: {
				item: {
					id: "item-read",
					sessionId: "session-1",
					turnId: "turn-1",
					revision: 1,
					seq: 2,
					state: "completed",
					item: {
						type: "toolResult",
						callId,
						isError: false,
						truncated: false,
						displayContent,
						output: {
							output: displayContent,
							preview: "def main",
							truncated: false,
						},
					},
				},
			},
		})

		let part: any
		const deadline = Date.now() + 1_000
		while (Date.now() < deadline) {
			const result = await Promise.race([
				stream.next(),
				new Promise<IteratorResult<any>>((resolve) =>
					setTimeout(() => resolve({ done: false, value: { payload: { type: "timeout" } } }), 25),
				),
			])
			const payload = result.value?.payload
			if (payload?.type === "message.part.updated") {
				const next = payload.properties.part
				if (next?.callID === callId) {
					part = next
					if (next.state?.status === "completed") break
				}
			}
			if (payload?.type === "timeout" && part) break
		}

		expect({
			output: part?.state?.output,
			hasRealNewline: typeof part?.state?.output === "string" && part.state.output.includes("\n"),
			notJsonBlob: typeof part?.state?.output === "string" && !part.state.output.trim().startsWith("{"),
		}).toEqual({
			output: displayContent,
			hasRealNewline: true,
			notJsonBlob: true,
		})
	})

	test("keeps the session busy after turn/start until turn/completed", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		await client.session.create()

		await client.session.promptAsync({
			sessionID: "session-1",
			parts: [{ type: "text", text: "hello" }],
		})

		expect((await client.session.status()).data["session-1"]).toEqual({ type: "busy" })

		transport.emit({
			type: "notification",
			method: "turn/completed",
			params: { turn: nativeTurnCompleted },
		})

		expect((await client.session.status()).data["session-1"]).toEqual({ type: "idle" })
	})

	test("session/list does not clear busy status while a turn is in flight", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		await client.session.create()

		await client.session.promptAsync({
			sessionID: "session-1",
			parts: [{ type: "text", text: "hello" }],
		})
		expect((await client.session.status()).data["session-1"]).toEqual({ type: "busy" })

		// Delete-refill and sidebar pagination re-list; durable snapshots stay Idle.
		await client.session.list({ limit: 5, roots: true })

		expect((await client.session.status()).data["session-1"]).toEqual({ type: "busy" })
	})

	test("projects context occupancy from context/usageUpdated", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		transport.emit({
			type: "notification",
			method: "context/usageUpdated",
			params: { sessionId: nativeSession.id, occupancy: nativeOccupancy },
		})

		expect(await nextPayloadOfType(stream, "context.usage.updated")).toEqual({
			type: "context.usage.updated",
			properties: {
				sessionID: nativeSession.id,
				occupancy: nativeOccupancy,
			},
		})
	})

	test("reads context occupancy through context/usage/read", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()

		const result = await client.context.usage.read({ sessionID: nativeSession.id })
		expect(result.data).toEqual(nativeOccupancy)
		expect(transport.requests.some((request) => request.method === "context/usage/read")).toBe(true)
		expect(await nextPayloadOfType(stream, "context.usage.updated")).toEqual({
			type: "context.usage.updated",
			properties: {
				sessionID: nativeSession.id,
				occupancy: nativeOccupancy,
			},
		})
	})

	test("preserves turn duration timestamps when loading session history", async () => {
		const transport = new FakeNativeTransport()
		transport.sessionItems = [
			{
				id: "item-user-1",
				sessionId: nativeSession.id,
				turnId: "turn-1",
				seq: 1,
				revision: 1,
				createdAt: "2026-08-24T00:00:00.000Z",
				updatedAt: "2026-08-24T00:00:00.000Z",
				state: "completed",
				item: {
					type: "userMessage",
					content: [{ type: "text", text: "hello" }],
					entry: "turnStart",
				},
			},
			{
				id: "item-assistant-1",
				sessionId: nativeSession.id,
				turnId: "turn-1",
				seq: 2,
				revision: 1,
				createdAt: "2026-08-24T00:00:02.000Z",
				updatedAt: "2026-08-24T00:00:14.000Z",
				state: "completed",
				item: {
					type: "assistantMessage",
					text: "world",
				},
			},
		]
		const client = createDevoClient({ directory: "/repo", transport })
		const { data } = await client.session.messages({ sessionID: nativeSession.id })
		const user = data.find((entry) => entry.info.role === "user")
		const assistant = data.find((entry) => entry.info.role === "assistant")
		expect(user?.info.time.created).toBe(Date.parse("2026-08-24T00:00:00.000Z"))
		expect(assistant?.info.time.created).toBe(Date.parse("2026-08-24T00:00:02.000Z"))
		expect(assistant?.info.time.completed).toBe(Date.parse("2026-08-24T00:00:14.000Z"))
		expect(
			(assistant?.info.time.completed ?? 0) - (user?.info.time.created ?? 0),
		).toBe(14_000)
	})

	test("preserves reasoning part duration when loading session history", async () => {
		const transport = new FakeNativeTransport()
		transport.sessionItems = [
			{
				id: "item-user-2",
				sessionId: nativeSession.id,
				turnId: "turn-2",
				seq: 1,
				revision: 1,
				createdAt: "2026-08-24T01:00:00.000Z",
				updatedAt: "2026-08-24T01:00:00.000Z",
				state: "completed",
				item: {
					type: "userMessage",
					content: [{ type: "text", text: "think" }],
					entry: "turnStart",
				},
			},
			{
				id: "item-reasoning-1",
				sessionId: nativeSession.id,
				turnId: "turn-2",
				seq: 2,
				revision: 1,
				createdAt: "2026-08-24T01:00:01.000Z",
				updatedAt: "2026-08-24T01:00:15.000Z",
				state: "completed",
				item: {
					type: "reasoning",
					text: "careful analysis",
				},
			},
		]
		const client = createDevoClient({ directory: "/repo", transport })
		const { data } = await client.session.messages({ sessionID: nativeSession.id })
		const reasoning = data.find((entry) =>
			entry.parts.some((part) => part.type === "reasoning"),
		)
		const part = reasoning?.parts.find((candidate) => candidate.type === "reasoning")
		expect(part?.time?.start).toBe(Date.parse("2026-08-24T01:00:01.000Z"))
		expect(part?.time?.end).toBe(Date.parse("2026-08-24T01:00:15.000Z"))
		expect((part?.time?.end ?? 0) - (part?.time?.start ?? 0)).toBe(14_000)
	})

	test("closes reasoning part interval from live started to completed", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		await client.session.create()

		transport.emit({
			type: "notification",
			method: "item/started",
			params: {
				item: {
					id: "item-reasoning-live",
					sessionId: nativeSession.id,
					turnId: "turn-1",
					seq: 1,
					revision: 1,
					createdAt: "2026-08-24T02:00:00.000Z",
					updatedAt: "2026-08-24T02:00:00.000Z",
					state: "running",
					item: { type: "reasoning", text: "" },
				},
			},
		})
		transport.emit({
			type: "notification",
			method: "item/completed",
			params: {
				item: {
					id: "item-reasoning-live",
					sessionId: nativeSession.id,
					turnId: "turn-1",
					seq: 1,
					revision: 2,
					createdAt: "2026-08-24T02:00:00.000Z",
					updatedAt: "2026-08-24T02:00:08.000Z",
					state: "completed",
					item: { type: "reasoning", text: "done thinking" },
				},
			},
		})

		const { data } = await client.session.messages({ sessionID: nativeSession.id })
		const reasoning = data.find((entry) => entry.info.id === "item-reasoning-live")
		const part = reasoning?.parts.find((candidate) => candidate.type === "reasoning")
		expect(part?.text).toBe("done thinking")
		expect(part?.time?.start).toBe(Date.parse("2026-08-24T02:00:00.000Z"))
		expect(part?.time?.end).toBe(Date.parse("2026-08-24T02:00:08.000Z"))
		expect(reasoning?.info.time.completed).toBe(Date.parse("2026-08-24T02:00:08.000Z"))
	})

	test("turn start/complete bumps lastActivity and emits session.updated for sidebar sync", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		await client.session.create()

		const before = (await client.session.get({ sessionID: nativeSession.id })).data
		expect(before?.time.lastActivity).toBe(Date.parse("2026-08-22T00:00:00Z"))

		transport.emit({
			type: "notification",
			method: "turn/started",
			params: { turn: nativeTurnInProgress },
		})
		const startedUpdate = await nextPayloadOfType(stream, "session.updated")
		expect(startedUpdate.properties.info.time.lastActivity).toBe(
			Date.parse("2026-08-24T00:00:00Z"),
		)

		transport.emit({
			type: "notification",
			method: "turn/completed",
			params: { turn: nativeTurnCompleted },
		})
		const completedUpdate = await nextPayloadOfType(stream, "session.updated")
		expect(completedUpdate.properties.info.time.lastActivity).toBe(
			Date.parse("2026-08-24T00:00:08Z"),
		)

		const after = (await client.session.get({ sessionID: nativeSession.id })).data
		expect(after?.time.lastActivity).toBe(Date.parse("2026-08-24T00:00:08Z"))
		expect(after?.time.updated).toBe(Date.parse("2026-08-24T00:00:08Z"))
	})

	test("loading session history emits the resume-enriched snapshot as session.updated", async () => {
		const transport = new FakeNativeTransport()
		transport.resumeSession = {
			...nativeSession,
			model: { provider: "test", model: "alt-model" },
			settings: { ...nativeSession.settings, reasoningEffort: "enabled", mode: "plan" },
		}
		const client = createDevoClient({ directory: "/repo", transport })
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		await client.session.messages({ sessionID: nativeSession.id })

		// The cold session/list snapshot carries the base model; resume is
		// authoritative for persisted per-session selections, so its enriched
		// snapshot must reach renderer session stores (they re-seed the
		// composer from session.updated) instead of staying client-internal.
		const update = await nextPayloadOfType(stream, "session.updated")
		expect(update.properties.session.model?.model).toBe("alt-model")
		expect(update.properties.session.settings?.reasoningEffort).toBe("enabled")
		expect(update.properties.session.settings?.mode).toBe("plan")
	})

	test("session/message/edit sends canonical params", async () => {
		const transport = new FakeNativeTransport()
		const client = createDevoClient({ directory: "/repo", transport })
		await client.session.create()
		await client.session.editMessage({
			sessionID: nativeSession.id,
			itemID: "item-user-1",
			text: "edited",
		})
		const request = transport.requests.find((entry) => entry.method === "session/message/edit")
		const params = (request?.params ?? {}) as {
			sessionId?: string
			itemId?: string
			expectedRevision?: number
			content?: unknown
			idempotencyKey?: string
		}
		expect({
			method: request?.method,
			sessionId: params.sessionId,
			itemId: params.itemId,
			expectedRevision: params.expectedRevision,
			content: params.content,
			hasIdempotencyKey: typeof params.idempotencyKey === "string" && params.idempotencyKey.length > 0,
		}).toEqual({
			method: "session/message/edit",
			sessionId: nativeSession.id,
			itemId: "item-user-1",
			expectedRevision: 0,
			content: [{ type: "text", text: "edited" }],
			hasIdempotencyKey: true,
		})
	})

	test("turn/superseded removes messages from the replaced turn", async () => {
		const transport = new FakeNativeTransport()
		transport.sessionItems = [
			{
				id: "item-user-1",
				sessionId: nativeSession.id,
				turnId: "turn-1",
				seq: 1,
				revision: 1,
				createdAt: "2026-08-24T00:00:00.000Z",
				updatedAt: "2026-08-24T00:00:00.000Z",
				state: "completed",
				item: {
					type: "userMessage",
					content: [{ type: "text", text: "hello" }],
					entry: "turnStart",
				},
			},
			{
				id: "item-assistant-1",
				sessionId: nativeSession.id,
				turnId: "turn-1",
				seq: 2,
				revision: 1,
				createdAt: "2026-08-24T00:00:02.000Z",
				updatedAt: "2026-08-24T00:00:14.000Z",
				state: "completed",
				item: {
					type: "assistantMessage",
					text: "world",
				},
			},
		]
		const client = createDevoClient({ directory: "/repo", transport })
		const loaded = await client.session.messages({ sessionID: nativeSession.id })
		expect(loaded.data.map((entry) => entry.info.id).sort()).toEqual([
			"item-assistant-1",
			"item-user-1",
		])
		const stream = (await client.global.event()).stream[Symbol.asyncIterator]()
		transport.emit({
			type: "notification",
			method: "turn/superseded",
			params: {
				sessionId: nativeSession.id,
				supersededTurnId: "turn-1",
				replacementTurnId: "turn-2",
				editId: "edit-1",
				reason: "message_edit_previous",
			},
		})
		expect(await nextPayloadOfType(stream, "message.removed")).toEqual({
			type: "message.removed",
			properties: { sessionID: nativeSession.id, messageID: "item-user-1" },
		})
		expect(await nextPayloadOfType(stream, "message.removed")).toEqual({
			type: "message.removed",
			properties: { sessionID: nativeSession.id, messageID: "item-assistant-1" },
		})
		const remaining = await client.session.messages({ sessionID: nativeSession.id })
		expect(remaining.data).toEqual([])
	})
})
