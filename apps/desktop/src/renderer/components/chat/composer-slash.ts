/**
 * First-party composer slash commands shared by the new-session and
 * in-session composers. Parsing lives here so both surfaces agree on
 * names; each surface decides what it can actually execute.
 */

export type ComposerSlashName =
	| "compact"
	| "fork"
	| "side"
	| "goal"
	| "plan"
	| "skills"
	| "research"

export type ParsedComposerSlash = {
	name: ComposerSlashName
	args: string
}

const FIRST_PARTY_NAMES = new Set<string>([
	"compact",
	"fork",
	"side",
	"btw",
	"goal",
	"plan",
	"skills",
	"research",
])

export function parseComposerSlash(text: string): ParsedComposerSlash | null {
	const trimmed = text.trim()
	if (!trimmed.startsWith("/")) return null
	const spaceIndex = trimmed.indexOf(" ")
	const rawName = (spaceIndex === -1 ? trimmed.slice(1) : trimmed.slice(1, spaceIndex)).toLowerCase()
	if (!FIRST_PARTY_NAMES.has(rawName)) return null
	const args = spaceIndex === -1 ? "" : trimmed.slice(spaceIndex + 1).trim()
	const name: ComposerSlashName = rawName === "btw" ? "side" : (rawName as ComposerSlashName)
	return { name, args }
}

/** Prefix the first user message when the Goal chip is active. */
export function goalPromptText(text: string): string {
	return `/goal ${text.trim()}`
}
