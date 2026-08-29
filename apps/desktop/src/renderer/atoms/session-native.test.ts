import { describe, expect, test } from "bun:test"
import { processEvent } from "./actions/event-processor"
import { partsFamily, partStorageKey } from "./parts"
import { sessionNativeFamily } from "./session-native"
import { sessionFamily, upsertSessionAtom } from "./sessions"
import { appStore } from "./store"
import { streamingVersionFamily } from "./streaming"

describe("Native session renderer state", () => {
	test("deduplicates replayed Native approvals by approval id", () => {
		const sessionID = "session-native-approval-dedup"
		appStore.set(upsertSessionAtom, {
			session: { id: sessionID, title: "Approval replay" },
			directory: "/repo",
		})
		const event = {
			type: "permission.asked",
			properties: {
				id: "approval-1",
				requestID: "approval-1",
				sessionID,
				permission: "Run command",
			},
		} as const

		processEvent(event)
		processEvent(event)

		expect(appStore.get(sessionFamily(sessionID))?.permissions).toEqual([event.properties])
	})

	test("stores command, config, mode, usage, and occupancy updates from events", () => {
		const sessionID = "session-native-state"

		processEvent({
			type: "session.commands.updated",
			properties: {
				sessionID,
				commands: [{ name: "compact", description: "Compact session" }],
			},
		})
		processEvent({
			type: "session.config.updated",
			properties: {
				sessionID,
				configOptions: [{ id: "model", currentValue: "test-model" }],
			},
		})
		processEvent({
			type: "session.mode.updated",
			properties: {
				sessionID,
				modeID: "plan",
			},
		})
		processEvent({
			type: "session.usage.updated",
			properties: {
				sessionID,
				used: 42,
				size: 100,
				cost: { amount: 1, currency: "USD" },
			},
		})
		processEvent({
			type: "context.usage.updated",
			properties: {
				sessionID,
				occupancy: {
					totalTokens: 48_000,
					contextWindowTokens: 190_000,
					categories: [
						{ id: "base", tokens: 8_000, shareBps: 1667 },
						{ id: "conversation", tokens: 40_000, shareBps: 8333 },
					],
				},
			},
		})

		expect(appStore.get(sessionNativeFamily(sessionID))).toEqual({
			commands: [{ name: "compact", description: "Compact session" }],
			configOptions: [{ id: "model", currentValue: "test-model" }],
			modeID: "plan",
			usage: {
				used: 42,
				size: 100,
				cost: { amount: 1, currency: "USD" },
			},
			occupancy: {
				totalTokens: 48_000,
				contextWindowTokens: 190_000,
				categories: [
					{ id: "base", tokens: 8_000, shareBps: 1667 },
					{ id: "conversation", tokens: 40_000, shareBps: 8333 },
				],
			},
		})
	})

	test("notifies session chat renders when text parts update", () => {
		const sessionID = "session-text-part-update"
		const messageID = "message-text-part-update"
		const initialVersion = appStore.get(streamingVersionFamily(sessionID))

		processEvent({
			type: "message.part.updated",
			properties: {
				part: {
					id: "message-text-part-update-text",
					sessionID,
					messageID,
					type: "text",
					text: "streamed text",
					time: { start: 1, end: 1 },
				},
			},
		})

		expect(appStore.get(partsFamily(partStorageKey(sessionID, messageID)))).toEqual([
			{
				id: "message-text-part-update-text",
				sessionID,
				messageID,
				type: "text",
				text: "streamed text",
				time: { start: 1, end: 1 },
			},
		])
		expect(appStore.get(streamingVersionFamily(sessionID))).toBe(initialVersion + 1)
	})

	test("stores scheduled retries, clears resumed retries, and reports transient failures", () => {
		const sessionID = "session-provider-retry"
		appStore.set(upsertSessionAtom, {
			session: { id: sessionID, title: "Retry test" },
			directory: "/repo",
		})

		processEvent({
			type: "turn.provider_retry_status",
			properties: {
				sessionID,
				turnID: "turn-1",
				attempt: 2,
				backoffMs: 1000,
				provider: "openai",
				model: "test-model",
				phase: "scheduled",
				message: "Retrying provider request in 1.0s",
			},
		})

		expect(appStore.get(sessionFamily(sessionID))?.retryStatus).toEqual({
			turnId: "turn-1",
			attempt: 2,
			backoffMs: 1000,
			provider: "openai",
			model: "test-model",
			phase: "scheduled",
			message: "Retrying provider request in 1.0s",
		})

		processEvent({
			type: "turn.provider_retry_status",
			properties: {
				sessionID,
				turnID: "turn-1",
				attempt: 2,
				backoffMs: 0,
				provider: "openai",
				model: "test-model",
				phase: "resumed",
				message: "Retrying provider request now",
			},
		})
		processEvent({
			type: "session.error",
			properties: {
				sessionID,
				error: {
					name: "PROVIDER_SERVER_ERROR",
					data: { message: "Internal server error" },
				},
			},
		})

		expect(appStore.get(sessionFamily(sessionID))).toEqual({
			session: { id: sessionID, title: "Retry test" },
			directory: "/repo",
			status: { type: "idle" },
			permissions: [],
			questions: [],
			retryStatus: undefined,
			error: {
				name: "PROVIDER_SERVER_ERROR",
				data: { message: "Internal server error" },
			},
		})
	})
})
