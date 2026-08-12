import { describe, expect, test } from "bun:test"
import {
	ProtocolValidationError,
	assertValidProtocolPayload,
} from "./protocol-validation"

describe("desktop protocol runtime validation", () => {
	test("accepts valid ACP session update notifications", () => {
		const payload = {
			sessionId: "s1",
			update: {
				sessionUpdate: "agent_message_chunk",
				content: { type: "text", text: "hello" },
			},
		}

		expect(
			assertValidProtocolPayload({
				direction: "incomingNotification",
				method: "session/update",
				payload,
			}),
		).toBe(payload)
	})

	test("rejects malformed ACP session update notifications", () => {
		expect(() =>
			assertValidProtocolPayload({
				direction: "incomingNotification",
				method: "session/update",
				payload: {
					update: {
						sessionUpdate: "agent_message_chunk",
						content: { type: "text", text: "hello" },
					},
				},
			}),
		).toThrow(ProtocolValidationError)
	})

	test("rejects malformed outgoing ACP prompt params", () => {
		expect(() =>
			assertValidProtocolPayload({
				direction: "outgoingRequest",
				method: "session/prompt",
				payload: {
					prompt: [{ type: "text", text: "missing session id" }],
				},
			}),
		).toThrow(/session\/prompt/)
	})

	test("validates incoming ACP results", () => {
		const payload = {
			sessions: [{ sessionId: "s1", cwd: "/repo" }],
		}

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
				payload: { sessions: [{ cwd: "/repo" }] },
			}),
		).toThrow(ProtocolValidationError)
	})

	test("validates workspace changes read requests and results", () => {
		const requestPayload = {
			session_id: "s1",
			scopes: ["turn"],
			turn_id: "t1",
			diff_detail: "full",
			max_diff_bytes: 2_000_000,
		}
		const resultPayload = {
			views: [
				{
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
					unified_diff: "diff --git a/src/main.rs b/src/main.rs\n",
					warnings: [],
					generated_at: "2026-06-26T00:00:00Z",
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
			session_id: "s1",
			turn_id: "t1",
			scope: "turn",
			status: "ready",
			coverage: "git_visible",
			change_set_status: "finalized",
			stats: { files_changed: 1, additions: 2, deletions: 1 },
			version: 1,
			generated_at: "2026-06-26T00:00:00Z",
		}

		expect(
			assertValidProtocolPayload({
				direction: "incomingNotification",
				method: "workspace/changes/updated",
				payload,
			}),
		).toBe(payload)
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
