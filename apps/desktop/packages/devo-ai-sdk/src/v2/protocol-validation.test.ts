import { describe, expect, test } from "bun:test"
import {
	ProtocolValidationError,
	assertValidProtocolPayload,
} from "./protocol-validation"

describe("desktop protocol runtime validation", () => {
	test("accepts valid Native session creation requests", () => {
		const payload = {
			cwd: "/repo",
			idempotencyKey: "new-session-1",
		}

		expect(
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "session/new",
				payload,
			}),
		).toBe(payload)
	})

	test("rejects malformed Native session creation requests", () => {
		expect(() =>
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "session/new",
				payload: { idempotencyKey: "missing-cwd" },
			}),
		).toThrow(ProtocolValidationError)
	})

	test("validates Native reverse approval request and response directions", () => {
		const request = {
			type: "approval",
			approvalId: "approval-1",
			targetItemId: null,
			actionSummary: "Run command",
			justification: "Needed for the task",
			resource: "ShellExec",
			availableScopes: ["once", "commandPrefixPersist"],
			commandPattern: ["cargo", "test"],
			commandPrefix: ["cargo", "test"],
			target: { kind: "command", command: "cargo test" },
			decision: null,
		}
		expect(
			assertValidProtocolPayload({
				direction: "incomingRequest",
				method: "approval/command/request",
				payload: request,
			}),
		).toBe(request)
		expect(() =>
			assertValidProtocolPayload({
				direction: "outgoingResponse",
				method: "approval/command/request",
				payload: { requestId: "approval-1" },
			}),
		).toThrow(ProtocolValidationError)
	})

	test("validates incoming Native results", () => {
		const payload = { data: [], nextCursor: null }

		expect(
			assertValidProtocolPayload({
				direction: "incomingResult",
				method: "session/list",
				payload,
			}),
		).toBe(payload)
		expect(() =>
			assertValidProtocolPayload({
				direction: "incomingResult",
				method: "session/list",
				payload: { sessions: [] },
			}),
		).toThrow(ProtocolValidationError)
	})

	test("validates workspace changes read requests and results", () => {
		const requestPayload = {
			sessionId: "s1",
			scopes: ["turn"],
			turnId: "t1",
			diffDetail: "full",
			maxDiffBytes: 2_000_000,
		}
		const resultPayload = {
			views: [
				{
					scope: "turn",
					status: "ready",
					workspaceRoot: "/repo",
					base: {
						kind: "turn_checkpoint",
						turn_id: "t1",
						checkpoint_id: "checkpoint-1",
						backend: "git_ghost_commit",
					},
					coverage: "git_visible",
					attribution: "workspace_net",
					changeSetStatus: "finalized",
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
					unifiedDiff: "diff --git a/src/main.rs b/src/main.rs\n",
					warnings: [],
					generatedAt: "2026-06-26T00:00:00Z",
				},
			],
		}

		expect(
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "workspace/changes/read",
				payload: requestPayload,
			}),
		).toBe(requestPayload)
		expect(
			assertValidProtocolPayload({
				direction: "incomingResult",
				method: "workspace/changes/read",
				payload: resultPayload,
			}),
		).toBe(resultPayload)
	})

	test("validates workspace changes updated notifications", () => {
		const payload = {
			sessionId: "s1",
			turnId: "t1",
			scope: "turn",
			status: "ready",
			coverage: "git_visible",
			changeSetStatus: "finalized",
			stats: { filesChanged: 1, additions: 2, deletions: 1 },
			version: 1,
			generatedAt: "2026-06-26T00:00:00Z",
		}

		expect(
			assertValidProtocolPayload({
				direction: "incomingNotification",
				method: "workspace/changes/updated",
				payload,
			}),
		).toBe(payload)
	})

	test("validates typed Native item notifications", () => {
		const payload = {
			item: {
				id: "item-1",
				sessionId: "session-1",
				turnId: "turn-1",
				seq: 1,
				revision: 1,
				createdAt: "2026-08-22T00:00:00Z",
				updatedAt: "2026-08-22T00:00:00Z",
				state: "running",
				item: { type: "assistantMessage", text: "hello" },
			},
		}

		expect(
			assertValidProtocolPayload({
				direction: "incomingNotification",
				method: "item/started",
				payload,
			}),
		).toBe(payload)
	})

	test("validates mcp tools requests and results", () => {
		const requestPayload = { name: "docs" }
		const resultPayload = {
			tools: [{ name: "get_time", description: "Current time" }],
		}
		expect(
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "mcp/tools",
				payload: requestPayload,
			}),
		).toBe(requestPayload)
		expect(
			assertValidProtocolPayload({
				direction: "incomingResult",
				method: "mcp/tools",
				payload: resultPayload,
			}),
		).toBe(resultPayload)
	})

	test("rejects unknown protocol methods", () => {
		expect(() =>
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "unknown/method",
				payload: {},
			}),
		).toThrow(/unknown protocol method/)
	})
})
