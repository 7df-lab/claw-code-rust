import type { PermissionRequest, PermissionResponse } from "../../lib/types"

export type ApprovalChoice =
	| {
			id: string
			kind: "approve"
			scope: PermissionResponse
			label: string
	  }
	| {
			id: string
			kind: "deny"
			label: string
	  }

const SCOPE_ALIASES: Record<string, PermissionResponse> = {
	once: "once",
	turn: "turn",
	session: "session",
	tool: "tool",
	pathprefix: "pathPrefix",
	path_prefix: "pathPrefix",
	host: "host",
	commandprefix: "commandPrefix",
	command_prefix: "commandPrefix",
	commandprefixpersist: "commandPrefixPersist",
	command_prefix_persist: "commandPrefixPersist",
}

function normalizeScopeToken(scope: unknown): PermissionResponse | null {
	if (typeof scope !== "string") return null
	const camel = scope.replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase())
	for (const candidate of [scope, camel]) {
		const key = candidate.toLowerCase()
		if (key in SCOPE_ALIASES) return SCOPE_ALIASES[key]
	}
	return null
}

function hasScope(scopes: PermissionResponse[], target: PermissionResponse): boolean {
	return scopes.includes(target)
}

function snippet(value: string, max = 48): string {
	const firstLine = value.split("\n").find((line) => line.trim())?.trim() ?? value.trim()
	const collapsed = value.includes("\n") ? `${firstLine}…` : firstLine
	return collapsed.length > max ? `${collapsed.slice(0, max)}…` : collapsed
}

function looksLikeToolCallId(value: string): boolean {
	const trimmed = value.trim()
	return trimmed.startsWith("call_") || trimmed.startsWith("call-")
}

/** Mirrors server `path_prefix_grant_root`: directory paths stay; files use parent. */
export function pathPrefixGrantRoot(path: string): string {
	const normalized = path.replace(/\\/g, "/")
	const lastSlash = normalized.lastIndexOf("/")
	if (lastSlash < 0) return path
	const lastSegment = normalized.slice(lastSlash + 1)
	const looksLikeFile = lastSegment.includes(".") && !lastSegment.endsWith(".")
	if (!looksLikeFile) return path
	return path.slice(0, lastSlash) || path
}

function availableScopesFor(permission: PermissionRequest): PermissionResponse[] {
	const raw = Array.isArray(permission.metadata?.availableScopes)
		? permission.metadata.availableScopes
		: ["once"]
	const normalized = raw
		.map(normalizeScopeToken)
		.filter((scope): scope is PermissionResponse => scope !== null)
	return Array.from(new Set(normalized))
}

/**
 * Mirrors `crates/tui/src/bottom_pane/approval_overlay.rs` option building:
 * once, contextual session/path/host/prefix-persist grants, then deny.
 * Does not surface turn/tool/ephemeral command-prefix scopes in the picker.
 */
export function buildApprovalChoices(permission: PermissionRequest): ApprovalChoice[] {
	const scopes = availableScopesFor(permission)
	const metadata = permission.metadata ?? {}
	const commandPattern = Array.isArray(metadata.commandPattern)
		? metadata.commandPattern.map(String)
		: undefined
	const commandPrefix = Array.isArray(metadata.commandPrefix)
		? metadata.commandPrefix.map(String)
		: undefined
	const target =
		typeof metadata.target === "string"
			? metadata.target
			: typeof metadata.command === "string"
				? metadata.command
				: undefined
	const path = typeof metadata.path === "string" ? metadata.path : undefined
	const host = typeof metadata.host === "string" ? metadata.host : undefined

	const choices: ApprovalChoice[] = []

	if (scopes.length === 0 || hasScope(scopes, "once")) {
		choices.push({
			id: "once",
			kind: "approve",
			scope: "once",
			label: "Allow once",
		})
	}

	if (hasScope(scopes, "session")) {
		const label = path
			? `Allow for this session · \`${snippet(path)}\``
			: commandPattern
				? `Allow for this session · \`${snippet(commandPattern.join(" "))}\``
				: target && !looksLikeToolCallId(target)
					? `Allow for this session · \`${snippet(target)}\``
					: "Allow for this session"
		choices.push({
			id: "session",
			kind: "approve",
			scope: "session",
			label,
		})
	}

	if (hasScope(scopes, "commandPrefixPersist") && commandPrefix) {
		choices.push({
			id: "commandPrefixPersist",
			kind: "approve",
			scope: "commandPrefixPersist",
			label: `Always allow commands starting with \`${snippet(commandPrefix.join(" "))}\``,
		})
	}

	if (hasScope(scopes, "pathPrefix") && path) {
		const root = pathPrefixGrantRoot(path)
		choices.push({
			id: "pathPrefix",
			kind: "approve",
			scope: "pathPrefix",
			label: `Allow files under \`${snippet(root)}\``,
		})
	}

	if (hasScope(scopes, "host") && host) {
		choices.push({
			id: "host",
			kind: "approve",
			scope: "host",
			label: `Allow \`${host}\` for this session`,
		})
	}

	choices.push({
		id: "deny",
		kind: "deny",
		label: "Deny",
	})

	return choices
}

export function permissionSummaryLines(permission: PermissionRequest): {
	title: string
	reason?: string
} {
	const justification =
		typeof permission.metadata?.justification === "string"
			? permission.metadata.justification.trim()
			: ""
	return {
		title: permission.permission,
		reason: justification.length > 0 ? justification : undefined,
	}
}
