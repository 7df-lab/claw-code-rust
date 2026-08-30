import type { MenuItemConstructorOptions } from "electron"
import type { Project, Session } from "@devo-ai/sdk/v2/client"
import { isWorkingSessionStatus } from "./notification-policy"
import type { SessionState } from "./notification-watcher"

const MAX_RUNNING_INLINE = 3
const MAX_RECENT_INLINE = 3
const MAX_TITLE_LENGTH = 48

export interface DiscoveryCache {
	projects: Project[]
	sessions: Session[]
}

export interface DevoTrayMenuOptions {
	liveSessions: ReadonlyMap<string, SessionState>
	discovery: DiscoveryCache | null
	pendingCount: number
	onNavigateToSession: (sessionId: string) => void
	onNewChat: () => void
	onOpenDevo: () => void
	onQuitDevo: () => void
}

interface TraySession {
	id: string
	title: string
	directory: string
	updatedAt: number
	totalInputTokens: number
	totalOutputTokens: number
	totalTokens: number
	totalCacheReadTokens: number
	parentId?: string
}

interface UsageSummary {
	inputTokens: number
	outputTokens: number
	totalTokens: number
	cacheReadTokens: number
}

export function buildDevoTrayMenuTemplate(
	options: DevoTrayMenuOptions,
): MenuItemConstructorOptions[] {
	const template: MenuItemConstructorOptions[] = []

	if (options.pendingCount > 0) {
		template.push({
			label: `${options.pendingCount} Pending ${
				options.pendingCount === 1 ? "Approval" : "Approvals"
			}`,
			click: options.onOpenDevo,
		})
		template.push(separator())
	}

	const discoverySessions = normalizeDiscoverySessions(options.discovery)
	const runningSection = buildRunningSection(
		options.liveSessions,
		discoverySessions,
		options.onNavigateToSession,
	)
	if (runningSection.length > 0) {
		template.push(...runningSection)
		template.push(separator())
	}

	const recentSection = buildRecentSection(
		options.liveSessions,
		discoverySessions,
		options.onNavigateToSession,
	)
	if (recentSection.length > 0) {
		template.push(...recentSection)
		template.push(separator())
	}

	template.push(...buildUsageSection(discoverySessions))
	template.push(separator())
	template.push({
		label: "New Chat",
		click: options.onNewChat,
	})
	template.push(separator())
	template.push({
		label: "Open Devo",
		click: options.onOpenDevo,
	})
	template.push(separator())
	template.push({
		label: "Quit Devo",
		click: options.onQuitDevo,
	})

	return template
}

function buildRunningSection(
	liveSessions: ReadonlyMap<string, SessionState>,
	discoverySessions: TraySession[],
	onNavigateToSession: (sessionId: string) => void,
): MenuItemConstructorOptions[] {
	const discoveryById = new Map(discoverySessions.map((session) => [session.id, session]))
	const runningSessions = Array.from(liveSessions.entries())
		.filter(([, state]) => !state.parentID && isWorkingSessionStatus(state.status))
		.map(([sessionId, state]) => {
			const discovered = discoveryById.get(sessionId)
			return {
				id: sessionId,
				title: titleForSession(state.title || discovered?.title),
				directory: state.directory || discovered?.directory || "",
				updatedAt: discovered?.updatedAt ?? 0,
				totalInputTokens: discovered?.totalInputTokens ?? 0,
				totalOutputTokens: discovered?.totalOutputTokens ?? 0,
				totalTokens: discovered?.totalTokens ?? 0,
				totalCacheReadTokens: discovered?.totalCacheReadTokens ?? 0,
			}
		})
		.sort((left, right) => right.updatedAt - left.updatedAt)

	if (runningSessions.length === 0) return []

	return buildSessionSection({
		header: "Running",
		sessions: runningSessions,
		maxInline: MAX_RUNNING_INLINE,
		onNavigateToSession,
	})
}

function buildRecentSection(
	liveSessions: ReadonlyMap<string, SessionState>,
	discoverySessions: TraySession[],
	onNavigateToSession: (sessionId: string) => void,
): MenuItemConstructorOptions[] {
	const runningSessionIds = new Set(
		Array.from(liveSessions.entries())
			.filter(([, state]) => !state.parentID && isWorkingSessionStatus(state.status))
			.map(([sessionId]) => sessionId),
	)
	const recentSessions = discoverySessions
		.filter((session) => !runningSessionIds.has(session.id))
		.filter((session) => !session.parentId)
		.sort((left, right) => right.updatedAt - left.updatedAt)

	if (recentSessions.length === 0) return []

	return buildSessionSection({
		header: "Recent",
		sessions: recentSessions,
		maxInline: MAX_RECENT_INLINE,
		onNavigateToSession,
	})
}

function buildSessionSection(options: {
	header: string
	sessions: TraySession[]
	maxInline: number
	onNavigateToSession: (sessionId: string) => void
}): MenuItemConstructorOptions[] {
	const visible = options.sessions.slice(0, options.maxInline)
	const overflow = options.sessions.slice(options.maxInline)
	const items: MenuItemConstructorOptions[] = [{ label: options.header, enabled: false }]

	for (const session of visible) {
		items.push(sessionMenuItem(session, options.onNavigateToSession))
	}

	if (overflow.length > 0) {
		items.push({
			label: "More",
			submenu: overflow.map((session) => sessionMenuItem(session, options.onNavigateToSession)),
		})
	}

	return items
}

function sessionMenuItem(
	session: TraySession,
	onNavigateToSession: (sessionId: string) => void,
): MenuItemConstructorOptions {
	return {
		label: truncateTitle(session.title),
		sublabel: projectNameFromDir(session.directory),
		click: () => onNavigateToSession(session.id),
	}
}

function buildUsageSection(sessions: TraySession[]): MenuItemConstructorOptions[] {
	const usage = summarizeUsage(sessions)
	return [
		{ label: "Usage", enabled: false },
		{ label: `Tokens ${formatTokenCount(usage.totalTokens)}`, enabled: false },
		{
			label: `Input ${formatTokenCount(usage.inputTokens)} · Output ${formatTokenCount(
				usage.outputTokens,
			)}`,
			enabled: false,
		},
		{ label: `Cache read ${formatTokenCount(usage.cacheReadTokens)}`, enabled: false },
	]
}

function summarizeUsage(sessions: TraySession[]): UsageSummary {
	return sessions.reduce(
		(summary, session) => ({
			inputTokens: summary.inputTokens + session.totalInputTokens,
			outputTokens: summary.outputTokens + session.totalOutputTokens,
			totalTokens: summary.totalTokens + session.totalTokens,
			cacheReadTokens: summary.cacheReadTokens + session.totalCacheReadTokens,
		}),
		{
			inputTokens: 0,
			outputTokens: 0,
			totalTokens: 0,
			cacheReadTokens: 0,
		},
	)
}

function normalizeDiscoverySessions(discovery: DiscoveryCache | null): TraySession[] {
	if (!discovery) return []

	return discovery.sessions.map((session) => {
		const totalInputTokens = numericSessionField(session, "totalInputTokens")
		const totalOutputTokens = numericSessionField(session, "totalOutputTokens")
		const totalTokens =
			numericSessionField(session, "totalTokens") || totalInputTokens + totalOutputTokens

		return {
			id: String(session.id),
			title: titleForSession(session.title),
			directory: String(session.directory ?? ""),
			updatedAt: Number(session.time?.updated ?? session.time?.created ?? 0),
			parentId: session.parentID ? String(session.parentID) : undefined,
			totalInputTokens,
			totalOutputTokens,
			totalTokens,
			totalCacheReadTokens: numericSessionField(session, "totalCacheReadTokens"),
		}
	})
}

function numericSessionField(session: Session, field: string): number {
	const value = session[field]
	return typeof value === "number" && Number.isFinite(value) ? value : 0
}

function titleForSession(title: unknown): string {
	return typeof title === "string" && title.trim() ? title : "New Chat"
}

function truncateTitle(title: string): string {
	if (title.length <= MAX_TITLE_LENGTH) return title
	return `${title.slice(0, MAX_TITLE_LENGTH - 1)}...`
}

function projectNameFromDir(directory: string): string {
	const parts = directory.split(/[\\/]/).filter(Boolean)
	return parts.at(-1) ?? "/"
}

function formatTokenCount(tokens: number): string {
	if (tokens >= 1_000_000) return `${formatOneDecimal(tokens / 1_000_000)}m`
	if (tokens >= 1_000) return `${formatOneDecimal(tokens / 1_000)}k`
	return String(tokens)
}

function formatOneDecimal(value: number): string {
	return value.toFixed(1).replace(/\.0$/, "")
}

function separator(): MenuItemConstructorOptions {
	return { type: "separator" }
}
