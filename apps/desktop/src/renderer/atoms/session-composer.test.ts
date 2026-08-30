import { describe, expect, test } from "bun:test"
import {
	composerFromMessages,
	composerFromPersistedModel,
	composerFromSessionModel,
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

describe("composerFromSessionModel", () => {
	const resolve = (seed: { provider?: string; model?: string }) => {
		if (seed.provider && seed.provider !== "unknown") {
			return { providerID: seed.provider, modelID: seed.model ?? "" }
		}
		return seed.model === "deepseek-v4-flash"
			? { providerID: "deepseek", modelID: seed.model }
			: null
	}

	test("builds composer state from the persisted session model", () => {
		expect(
			composerFromSessionModel(
				{ model: "deepseek-v4-flash", reasoningEffort: "high" },
				resolve,
			),
		).toEqual({
			model: { providerID: "deepseek", modelID: "deepseek-v4-flash" },
			variant: "high",
			agent: null,
			hasUserOverride: false,
		})
	})

	test("uses the wire provider id when present", () => {
		const seeded = composerFromSessionModel({ provider: "openai", model: "custom-model" }, resolve)
		expect(seeded?.model).toEqual({ providerID: "openai", modelID: "custom-model" })
	})

	test("returns null when the slug resolves to no known provider", () => {
		expect(composerFromSessionModel({ model: "gone-model" }, resolve)).toBeNull()
		expect(composerFromSessionModel(undefined, resolve)).toBeNull()
	})
})

describe("hydrateSessionComposerState with persisted session seed", () => {
	test("uses the session seed when history messages carry no model metadata", () => {
		const messages: Message[] = [{ id: "m1", role: "user", content: "hello" } as Message]
		const seed: SessionComposerState = {
			model: { providerID: "deepseek", modelID: "deepseek-v4-flash" },
			variant: "high",
			agent: null,
			hasUserOverride: false,
		}
		expect(
			hydrateSessionComposerState(EMPTY_COMPOSER_STATE, messages, undefined, seed),
		).toEqual(seed)
	})

	test("current session settings win over older message metadata", () => {
		const messages: Message[] = [
			{
				id: "m1",
				role: "user",
				content: "hello",
				model: { providerID: "openai", modelID: "gpt-4" },
			} as Message,
		]
		const seed: SessionComposerState = {
			model: { providerID: "deepseek", modelID: "deepseek-v4-flash" },
			variant: undefined,
			agent: null,
			hasUserOverride: false,
		}
		const hydrated = hydrateSessionComposerState(EMPTY_COMPOSER_STATE, messages, undefined, seed)
		expect(hydrated).toEqual(seed)
	})

	test("keeps empty composer for non-empty sessions without metadata or seed", () => {
		const messages: Message[] = [{ id: "m1", role: "user", content: "hello" } as Message]
		const hydrated = hydrateSessionComposerState(EMPTY_COMPOSER_STATE, messages, {
			providerID: "anthropic",
			modelID: "claude-3",
		})
		expect(hydrated.model).toBeNull()
	})
})
