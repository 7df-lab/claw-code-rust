import { describe, expect, test } from "bun:test"
import {
	composerFromMessages,
	composerFromPersistedModel,
	EMPTY_COMPOSER_STATE,
	hydrateSessionComposerState,
	type SessionComposerState,
} from "./session-composer"
import type { Message } from "../lib/types"

describe("hydrateSessionComposerState", () => {
	test("prefers last user message model over project default", () => {
		const messages: Message[] = [
			{
				id: "m1",
				role: "user",
				content: "hello",
				model: { providerID: "openai", modelID: "gpt-4" },
				variant: "high",
			} as Message,
		]
		const hydrated = hydrateSessionComposerState(EMPTY_COMPOSER_STATE, messages, {
			providerID: "anthropic",
			modelID: "claude-3",
		})
		expect(hydrated).toEqual({
			model: { providerID: "openai", modelID: "gpt-4" },
			variant: "high",
			agent: null,
			hasUserOverride: false,
		})
	})

	test("falls back to project default for empty sessions", () => {
		const hydrated = hydrateSessionComposerState(EMPTY_COMPOSER_STATE, [], {
			providerID: "anthropic",
			modelID: "claude-3",
			variant: "medium",
			agent: "build",
		})
		expect(hydrated).toEqual({
			model: { providerID: "anthropic", modelID: "claude-3" },
			variant: "medium",
			agent: "build",
			hasUserOverride: false,
		})
	})

	test("does not overwrite user overrides", () => {
		const current: SessionComposerState = {
			model: { providerID: "openai", modelID: "gpt-4o" },
			variant: undefined,
			agent: null,
			hasUserOverride: true,
		}
		const messages: Message[] = [
			{
				id: "m1",
				role: "user",
				content: "hello",
				model: { providerID: "openai", modelID: "gpt-4" },
			} as Message,
		]
		expect(hydrateSessionComposerState(current, messages, undefined)).toEqual(current)
	})
})

describe("composerFromMessages", () => {
	test("returns null when no user messages carry composer metadata", () => {
		const messages: Message[] = [{ id: "m1", role: "assistant", content: "hi" } as Message]
		expect(composerFromMessages(messages)).toBeNull()
	})
})

describe("composerFromPersistedModel", () => {
	test("returns empty state when project default is missing", () => {
		expect(composerFromPersistedModel(undefined)).toEqual(EMPTY_COMPOSER_STATE)
	})
})
