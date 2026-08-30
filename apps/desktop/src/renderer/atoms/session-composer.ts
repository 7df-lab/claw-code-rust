import { atom } from "jotai"
import { atomFamily } from "jotai/utils"
import type { ModelRef } from "../hooks/use-devo-data"
import type { Message } from "../lib/types"
import type { PersistedModelRef } from "./preferences"

export interface SessionComposerState {
	model: ModelRef | null
	variant?: string
	agent: string | null
	/** Set when the user explicitly changes composer settings for this session. */
	hasUserOverride: boolean
}

const EMPTY_COMPOSER_STATE: SessionComposerState = {
	model: null,
	variant: undefined,
	agent: null,
	hasUserOverride: false,
}

export { EMPTY_COMPOSER_STATE }

export const sessionComposerFamily = atomFamily((_sessionId: string) =>
	atom<SessionComposerState>(EMPTY_COMPOSER_STATE),
)

export function composerFromPersistedModel(
	stored: PersistedModelRef | undefined,
): SessionComposerState {
	if (!stored?.providerID || !stored?.modelID) {
		return EMPTY_COMPOSER_STATE
	}
	return {
		model: { providerID: stored.providerID, modelID: stored.modelID },
		variant: stored.variant,
		agent: stored.agent ?? null,
		hasUserOverride: false,
	}
}

export interface SessionModelSeed {
	provider?: string
	model?: string
	reasoningEffort?: string
}

/**
 * Builds composer state from the persisted wire-session model settings.
 * `resolveModel` maps the seed to a full ModelRef — preferring the wire
 * provider id (`session/resume` carries a real one) and falling back to a
 * reverse slug lookup across providers (cold `session/list` snapshots may
 * only know `"unknown"`).
 */
export function composerFromSessionModel(
	seed: SessionModelSeed | null | undefined,
	resolveModel: (seed: SessionModelSeed) => ModelRef | null,
): SessionComposerState | null {
	if (!seed?.model) return null
	const model = resolveModel(seed)
	if (!model) return null
	return {
		model,
		variant: seed.reasoningEffort,
		agent: null,
		hasUserOverride: false,
	}
}

export function composerFromMessages(messages: Message[]): SessionComposerState | null {
	for (let index = messages.length - 1; index >= 0; index--) {
		const message = messages[index]
		if (message.role !== "user") continue
		const dynamic = message as Message & Record<string, unknown>
		let model: ModelRef | null = null
		if ("model" in message && message.model) {
			const raw = message.model as { providerID: string; modelID: string }
			if (raw.providerID && raw.modelID) {
				model = { providerID: raw.providerID, modelID: raw.modelID }
			}
		}
		const variant =
			typeof dynamic.variant === "string" && dynamic.variant.length > 0
				? dynamic.variant
				: undefined
		const agentName =
			typeof dynamic.agent === "string" && dynamic.agent.length > 0 ? dynamic.agent : null
		if (model || variant || agentName) {
			return {
				model,
				variant,
				agent: agentName,
				hasUserOverride: false,
			}
		}
	}
	return null
}

export function hydrateSessionComposerState(
	current: SessionComposerState,
	messages: Message[],
	projectDefault: PersistedModelRef | undefined,
	/** Persisted per-session turn settings from the wire session (server restores them). */
	sessionSeed?: SessionComposerState | null,
): SessionComposerState {
	if (current.hasUserOverride) return current
	// The session snapshot is the current "next turn" configuration. It is
	// newer and authoritative over model metadata from an older history item;
	// history remains the fallback for legacy sessions without a snapshot seed.
	if (sessionSeed) return sessionSeed
	const fromMessages = composerFromMessages(messages)
	if (fromMessages) return fromMessages
	if (messages.length > 0) return current
	return composerFromPersistedModel(projectDefault)
}

export const setSessionComposerAtom = atom(
	null,
	(
		_get,
		set,
		args: {
			sessionId: string
			patch: Partial<SessionComposerState>
			userOverride?: boolean
		},
	) => {
		set(sessionComposerFamily(args.sessionId), (current) => ({
			...current,
			...args.patch,
			hasUserOverride: args.userOverride ? true : current.hasUserOverride,
		}))
	},
)
