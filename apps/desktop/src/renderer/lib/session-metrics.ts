/**
 * Pure utility functions for computing session timing, cost, and token metrics.
 *
 * All functions operate on SDK Message types and produce formatted strings or
 * numeric totals. No atoms or React dependencies -- safe to use anywhere.
 */

import type { ChatTurn } from "../atoms/derived/session-chat"
import type { ToolCategory } from "../components/chat/tool-category"
import type { AssistantMessage, Message, Part } from "./types"

// ============================================================
// Types
// ============================================================

export interface SessionTokens {
	input: number
	output: number
	reasoning: number
	cacheRead: number
	cacheWrite: number
	total: number
}

/** Distribution of turns across models (modelID -> count) */
export type ModelDistribution = Record<string, number>

/** Distribution of tool calls across categories (ToolCategory -> count) */
export type ToolBreakdown = Partial<Record<ToolCategory, number>>

export interface SessionMetrics {
	/** Total agent work time in milliseconds */
	workTimeMs: number
	/** Work time from completed messages only (excludes in-progress) */
	completedWorkTimeMs: number
	/** Start time (epoch ms) of the in-progress assistant message, or null if idle */
	activeStartMs: number | null
	/** Total cost in USD */
	cost: number
	/** Aggregated token counts */
	tokens: SessionTokens
	/** Number of exchanges (one exchange = one user message + all its assistant responses) */
	exchangeCount: number
	/** Number of user messages in the session */
	userMessageCount: number
	/** Number of assistant messages (LLM invocations) in the session */
	assistantMessageCount: number
	/** Map of modelID -> number of assistant messages using that model */
	modelDistribution: ModelDistribution
	/** Cache hit ratio: cacheRead / (input + cacheRead), as a percentage 0-100 */
	cacheEfficiency: number
	/** Number of assistant messages that had an error */
	errorCount: number
	/** Average cost per exchange (USD) */
	avgExchangeCost: number
	/** Average work time per exchange (ms) */
	avgExchangeTimeMs: number
}

/** Extended metrics that include parts-derived data (tool breakdown, retry count) */
export interface SessionMetricsExtended extends SessionMetrics {
	/** Tool calls by category (explore, edit, run, delegate, etc.) */
	toolBreakdown: ToolBreakdown
	/** Total number of tool calls */
	toolCallCount: number
	/** Number of retry attempts (from RetryPart) */
	retryCount: number
}

const MAX_COMPLETED_TURN_WORK_TIME_MS = 24 * 60 * 60 * 1000
const MIN_PLAUSIBLE_EPOCH_MS = Date.UTC(2020, 0, 1)
const MAX_FUTURE_SKEW_MS = 5 * 60 * 1000

// ============================================================
// Extraction helpers
// ============================================================

function isAssistantMessage(msg: Message): msg is AssistantMessage {
	return msg.role === "assistant"
}

/** Extract all assistant messages from a mixed message array. */
export function getAssistantMessages(messages: Message[]): AssistantMessage[] {
	return messages.filter(isAssistantMessage)
}

// ============================================================
// Work time computation
// ============================================================

/**
 * Compute total agent work time across all assistant messages.
 * Sums `(completed - created)` for each assistant message that has completed.
 * Messages still in progress (no `completed` timestamp) are included with
 * `Date.now()` as the end time.
 */
export function computeAgentWorkTime(messages: Message[]): number {
	let total = 0
	const now = Date.now()
	for (const msg of messages) {
		if (msg.role !== "assistant") continue
		if (msg.time.completed != null) {
			const elapsed = Math.max(0, msg.time.completed - msg.time.created)
			if (elapsed <= MAX_COMPLETED_TURN_WORK_TIME_MS) total += elapsed
		} else if (isPlausibleEpochMs(msg.time.created, now)) {
			total += Math.max(0, now - msg.time.created)
		}
	}
	return total
}

function isPlausibleEpochMs(timestamp: number, nowMs: number): boolean {
	return (
		Number.isFinite(timestamp) &&
		timestamp >= MIN_PLAUSIBLE_EPOCH_MS &&
		timestamp <= nowMs + MAX_FUTURE_SKEW_MS
	)
}

function partTimestamp(part: Part): number | undefined {
	if (part.type === "tool") {
		const time = part.state?.time
		return typeof time?.end === "number"
			? time.end
			: typeof time?.start === "number"
				? time.start
				: undefined
	}

	const time = part.time
	return typeof time?.end === "number"
		? time.end
		: typeof time?.start === "number"
			? time.start
			: undefined
}

function completedTurnEnd(turn: ChatTurn): number | undefined {
	for (let i = turn.assistantMessages.length - 1; i >= 0; i--) {
		const completed = turn.assistantMessages[i].info.time.completed
		if (typeof completed === "number") return completed
	}

	let fallback: number | undefined
	for (const entry of turn.assistantMessages) {
		if (typeof entry.info.time.created === "number") {
			fallback =
				fallback === undefined ? entry.info.time.created : Math.max(fallback, entry.info.time.created)
		}
		for (const part of entry.parts) {
			const timestamp = partTimestamp(part)
			if (timestamp !== undefined) {
				fallback = fallback === undefined ? timestamp : Math.max(fallback, timestamp)
			}
		}
	}
	return fallback
}

function terminalPartTimestamp(part: Part): number | undefined {
	if (part.type === "tool") {
		const end = part.state?.time?.end
		return typeof end === "number" ? end : undefined
	}

	const end = part.time?.end
	return typeof end === "number" ? end : undefined
}

function completedTurnWorkEnd(turn: ChatTurn): number | undefined {
	let end: number | undefined

	for (const entry of turn.assistantMessages) {
		const completed = entry.info.time.completed
		if (typeof completed === "number") {
			end = end === undefined ? completed : Math.max(end, completed)
		}
		for (const part of entry.parts) {
			const timestamp = terminalPartTimestamp(part)
			if (timestamp !== undefined) {
				end = end === undefined ? timestamp : Math.max(end, timestamp)
			}
		}
	}

	if (end !== undefined) return end
	return completedTurnEnd(turn)
}

function completedTurnStopTime(turn: ChatTurn): number | undefined {
	for (let i = turn.assistantMessages.length - 1; i >= 0; i--) {
		const completed = turn.assistantMessages[i].info.time.completed
		if (typeof completed === "number") return completed
	}

	let fallback: number | undefined
	for (const entry of turn.assistantMessages) {
		for (const part of entry.parts) {
			const timestamp = terminalPartTimestamp(part)
			if (timestamp !== undefined) {
				fallback = fallback === undefined ? timestamp : Math.max(fallback, timestamp)
			}
		}
	}
	return fallback
}

/**
 * Compute end-to-end elapsed time for a single turn.
 * Starts at the user message creation time and ends at the turn completion time.
 * Active turns may use `Date.now()`; completed turns fall back to persisted
 * message/part timestamps so historical durations do not keep growing.
 */
export function computeTurnWorkTime(
	turn: ChatTurn,
	options: { active?: boolean; now?: () => number } = {},
): number {
	const start = turn.userMessage.info.time.created
	const end = options.active ? (options.now?.() ?? Date.now()) : completedTurnWorkEnd(turn)
	if (typeof start !== "number" || typeof end !== "number") return 0
	const elapsed = Math.max(0, end - start)
	if (!options.active && elapsed > MAX_COMPLETED_TURN_WORK_TIME_MS) return 0
	return elapsed
}

/**
 * Compute elapsed time for a single reasoning/thought part.
 * Active thoughts use `Date.now()`; completed thoughts require both
 * `time.start` and `time.end`. Returns 0 when the interval is missing or
 * implausible (legacy history without distinct start/end).
 */
export function computeThoughtWorkTime(
	part: { time?: { start?: number; end?: number } },
	options: { active?: boolean; now?: () => number } = {},
): number {
	const start = part.time?.start
	if (typeof start !== "number" || !Number.isFinite(start)) return 0
	const end = options.active
		? (options.now?.() ?? Date.now())
		: typeof part.time?.end === "number" && Number.isFinite(part.time.end)
			? part.time.end
			: undefined
	if (typeof end !== "number") return 0
	const elapsed = Math.max(0, end - start)
	if (!options.active && elapsed > MAX_COMPLETED_TURN_WORK_TIME_MS) return 0
	if (!options.active && elapsed <= 0) return 0
	return elapsed
}

/**
 * Compute the live elapsed-time split for a single active turn.
 * The submit button timer uses the same user-message start as completed turn
 * duration, so active and completed displays share one semantic.
 */
export function computeTurnWorkTimeSplit(turn: ChatTurn): {
	completedMs: number
	activeStartMs: number | null
} {
	return { completedMs: 0, activeStartMs: turn.userMessage.info.time.created ?? null }
}

export type LatestTurnTimerMode = "running" | "stopped"

/**
 * Compute the top-bar timer split for the latest turn only.
 * This intentionally ignores earlier turns so each user message resets the
 * app-bar timer, while session-level metrics can still remain cumulative.
 */
export function computeLatestTurnTimerSplit(
	turns: ChatTurn[],
	options: {
		mode: LatestTurnTimerMode
		now?: () => number
		fallbackCompletedMs?: number
	},
): { completedMs: number; activeStartMs: number | null } {
	const turn = turns.at(-1)
	if (!turn) return { completedMs: 0, activeStartMs: null }

	switch (options.mode) {
		case "running": {
			const start = turn.userMessage.info.time.created
			const now = options.now?.() ?? Date.now()
			if (typeof start === "number" && isPlausibleEpochMs(start, now)) {
				return { completedMs: 0, activeStartMs: start }
			}
			return { completedMs: 0, activeStartMs: null }
		}
		case "stopped": {
			const fallback =
				typeof options.fallbackCompletedMs === "number" &&
				Number.isFinite(options.fallbackCompletedMs)
					? Math.max(0, options.fallbackCompletedMs)
					: 0
			const start = turn.userMessage.info.time.created
			const end = completedTurnStopTime(turn)
			if (typeof start === "number" && typeof end === "number") {
				const completedMs = Math.max(0, end - start)
				if (completedMs <= MAX_COMPLETED_TURN_WORK_TIME_MS) {
					return { completedMs, activeStartMs: null }
				}
			}
			return {
				completedMs: fallback,
				activeStartMs: null,
			}
		}
	}
}

/**
 * Compute the cost for a single turn by summing assistant message costs.
 */
export function computeTurnCost(turn: ChatTurn): number {
	let total = 0
	for (const entry of turn.assistantMessages) {
		if (entry.info.role === "assistant") {
			total += entry.info.cost ?? 0
		}
	}
	return total
}

// ============================================================
// Cost computation
// ============================================================

/** Sum the cost field across all assistant messages. */
export function computeSessionCost(messages: Message[]): number {
	let total = 0
	for (const msg of messages) {
		if (msg.role === "assistant") {
			total += msg.cost ?? 0
		}
	}
	return total
}

// ============================================================
// Token computation
// ============================================================

/** Sum token counts across all assistant messages. */
export function computeSessionTokens(messages: Message[]): SessionTokens {
	const result: SessionTokens = {
		input: 0,
		output: 0,
		reasoning: 0,
		cacheRead: 0,
		cacheWrite: 0,
		total: 0,
	}

	for (const msg of messages) {
		if (msg.role !== "assistant") continue
		const t = msg.tokens
		if (!t) continue
		result.input += t.input ?? 0
		result.output += t.output ?? 0
		result.reasoning += t.reasoning ?? 0
		result.cacheRead += t.cache?.read ?? 0
		result.cacheWrite += t.cache?.write ?? 0
	}

	result.total =
		result.input + result.output + result.reasoning + result.cacheRead + result.cacheWrite
	return result
}

// ============================================================
// Full session metrics computation (single pass over messages)
// ============================================================

/**
 * Compute all session metrics at once (work time + cost + tokens + model
 * distribution + cache efficiency + error count + turn averages).
 * Iterates the message array only once for efficiency.
 */
export function computeSessionMetrics(messages: Message[]): SessionMetrics {
	let workTimeMs = 0
	let completedWorkTimeMs = 0
	let activeStartMs: number | null = null
	const now = Date.now()
	let cost = 0
	let userMessageCount = 0
	let assistantMessageCount = 0
	let errorCount = 0
	const modelDistribution: ModelDistribution = {}
	const tokens: SessionTokens = {
		input: 0,
		output: 0,
		reasoning: 0,
		cacheRead: 0,
		cacheWrite: 0,
		total: 0,
	}

	// Track which user messages have at least one assistant response to count exchanges.
	// An exchange = one user message that triggered at least one assistant response.
	const userIdsWithResponses = new Set<string>()

	for (const msg of messages) {
		if (msg.role === "user") {
			userMessageCount++
			continue
		}

		// assistant message
		assistantMessageCount++

		// Track the parent user message for exchange counting
		if (msg.parentID) {
			userIdsWithResponses.add(msg.parentID)
		}

		// Work time. History replay uses small ordering timestamps (1, 2, 3...)
		// for stable sorting; those are not wall-clock epoch values and must not
		// be treated as active timers.
		if (msg.time.completed != null) {
			const elapsed = Math.max(0, msg.time.completed - msg.time.created)
			if (elapsed <= MAX_COMPLETED_TURN_WORK_TIME_MS) {
				workTimeMs += elapsed
				completedWorkTimeMs += elapsed
			}
		} else if (isPlausibleEpochMs(msg.time.created, now)) {
			workTimeMs += Math.max(0, now - msg.time.created)
			activeStartMs = msg.time.created
		}

		// Cost
		cost += msg.cost ?? 0

		// Model distribution
		if (msg.modelID) {
			modelDistribution[msg.modelID] = (modelDistribution[msg.modelID] ?? 0) + 1
		}

		// Error count
		if (msg.error) {
			errorCount++
		}

		// Tokens
		const t = msg.tokens
		if (t) {
			tokens.input += t.input ?? 0
			tokens.output += t.output ?? 0
			tokens.reasoning += t.reasoning ?? 0
			tokens.cacheRead += t.cache?.read ?? 0
			tokens.cacheWrite += t.cache?.write ?? 0
		}
	}

	tokens.total =
		tokens.input + tokens.output + tokens.reasoning + tokens.cacheRead + tokens.cacheWrite

	// Cache efficiency: how much of the input was served from cache
	const totalInput = tokens.input + tokens.cacheRead
	const cacheEfficiency = totalInput > 0 ? (tokens.cacheRead / totalInput) * 100 : 0

	// Exchange count: number of user messages that got at least one assistant response.
	// Falls back to userMessageCount if parentID tracking is unavailable.
	const exchangeCount = userIdsWithResponses.size > 0 ? userIdsWithResponses.size : userMessageCount

	// Per-exchange averages
	const avgExchangeCost = exchangeCount > 0 ? cost / exchangeCount : 0
	const avgExchangeTimeMs = exchangeCount > 0 ? workTimeMs / exchangeCount : 0

	return {
		workTimeMs,
		completedWorkTimeMs,
		activeStartMs,
		cost,
		tokens,
		exchangeCount,
		userMessageCount,
		assistantMessageCount,
		modelDistribution,
		cacheEfficiency,
		errorCount,
		avgExchangeCost,
		avgExchangeTimeMs,
	}
}

// ============================================================
// Parts-derived metrics (tool breakdown + retry count)
// ============================================================

/**
 * Compute parts-derived metrics: tool usage breakdown and retry count.
 * Requires a `getCategory` function to avoid importing UI-layer code.
 *
 * @param allParts - Flat array of all parts across all messages in the session
 * @param getCategory - Maps a tool name to its ToolCategory
 */
export function computePartsMetrics(
	allParts: Part[],
	getCategory: (tool: string) => ToolCategory,
): { toolBreakdown: ToolBreakdown; toolCallCount: number; retryCount: number } {
	const toolBreakdown: ToolBreakdown = {}
	let toolCallCount = 0
	let retryCount = 0

	for (const part of allParts) {
		if (part.type === "tool") {
			toolCallCount++
			const cat = getCategory(part.tool)
			toolBreakdown[cat] = (toolBreakdown[cat] ?? 0) + 1
		} else if (part.type === "retry") {
			retryCount++
		}
	}

	return { toolBreakdown, toolCallCount, retryCount }
}

/**
 * Compute extended session metrics combining message-level and parts-level data.
 */
export function computeSessionMetricsExtended(
	messages: Message[],
	allParts: Part[],
	getCategory: (tool: string) => ToolCategory,
): SessionMetricsExtended {
	const base = computeSessionMetrics(messages)
	const partsMetrics = computePartsMetrics(allParts, getCategory)
	return { ...base, ...partsMetrics }
}

// ============================================================
// Context window usage (last message vs. model limit)
// ============================================================

export interface ContextUsage {
	/** Total tokens from the last assistant message */
	lastMessageTokens: number
	/** Model context window limit (from provider data) */
	contextLimit: number
	/** Usage percentage 0-100 */
	percentage: number
	/** Provider ID of the last assistant message */
	providerID: string
	/** Model ID of the last assistant message */
	modelID: string
	/** Token count at which compaction will trigger (null if unknown) */
	compactionThreshold: number | null
	/** Usage percentage toward compaction threshold 0-100 (null if unknown) */
	compactionPercentage: number | null
}

/** Default buffer reserved for output tokens before compaction. */
const COMPACTION_BUFFER = 20_000

/** Maximum output tokens Devo will request (capped at this value). */
const OUTPUT_TOKEN_MAX = 32_000

/**
 * Compute the compaction threshold for a model, mirroring the logic from
 * devo's `SessionCompaction.isOverflow`. Returns the usable token count
 * above which compaction will be triggered.
 *
 * @param limit - The model's token limits
 * @param configReserved - Optional override from `config.compaction.reserved`
 */
export function computeCompactionThreshold(
	limit: { context: number; input?: number; output: number },
	configReserved?: number,
): number {
	const maxOutput = Math.min(limit.output, OUTPUT_TOKEN_MAX) || OUTPUT_TOKEN_MAX
	const reserved = configReserved ?? Math.min(COMPACTION_BUFFER, maxOutput)
	return limit.input ? limit.input - reserved : limit.context - maxOutput
}

/** Model limit info returned by the lookup callback. */
export interface ModelLimitInfo {
	context: number
	input?: number
	output: number
}

/** Compaction config from the Devo server, used to compute accurate thresholds. */
export interface CompactionOptions {
	/** Whether auto-compaction is enabled (default: true) */
	auto?: boolean
	/** User-configured reserved token buffer (overrides the default 20k) */
	reserved?: number
}

/**
 * Compute context window usage from the **last** assistant message that has
 * token data. This reflects the current state of the context window (how
 * full it is right now), NOT the cumulative session total.
 *
 * Returns `null` if there are no assistant messages with tokens, or if
 * the model's context limit is unavailable.
 *
 * @param messages - All messages in the session
 * @param getModelLimit - Callback to look up a model's limit info
 *   given `(providerID, modelID)`. Returns `undefined` if unknown.
 * @param compaction - Optional compaction config from the server
 */
export function computeContextUsage(
	messages: Message[],
	getModelLimit: (providerID: string, modelID: string) => ModelLimitInfo | undefined,
	compaction?: CompactionOptions,
): ContextUsage | null {
	// Find the last assistant message with token data (walking backwards)
	for (let i = messages.length - 1; i >= 0; i--) {
		const msg = messages[i]
		if (msg.role !== "assistant") continue
		const t = msg.tokens
		if (!t) continue

		const total =
			(t.input ?? 0) +
			(t.output ?? 0) +
			(t.reasoning ?? 0) +
			(t.cache?.read ?? 0) +
			(t.cache?.write ?? 0)
		if (total <= 0) continue

		const limit = getModelLimit(msg.providerID, msg.modelID)
		if (!limit || limit.context <= 0) return null

		// Only compute compaction if auto-compaction is not explicitly disabled
		const compactionEnabled = compaction?.auto !== false
		const threshold = compactionEnabled
			? computeCompactionThreshold(limit, compaction?.reserved)
			: null

		return {
			lastMessageTokens: total,
			contextLimit: limit.context,
			percentage: Math.round((total / limit.context) * 100),
			providerID: msg.providerID,
			modelID: msg.modelID,
			compactionThreshold: threshold != null && threshold > 0 ? threshold : null,
			compactionPercentage:
				threshold != null && threshold > 0 ? Math.round((total / threshold) * 100) : null,
		}
	}
	return null
}

// ============================================================
// Context breakdown estimation
// ============================================================

export type ContextBreakdownKey = "system" | "user" | "assistant" | "tool" | "other"

export interface ContextBreakdownSegment {
	key: ContextBreakdownKey
	tokens: number
	/** Width percentage 0-100 (for rendering a stacked bar) */
	width: number
	/** Display percentage rounded to 1 decimal */
	percent: number
}

/** Rough estimate: ~4 chars per token (same heuristic as devo). */
const estimateTokens = (chars: number) => Math.ceil(chars / 4)

/**
 * Estimate a breakdown of input tokens by role: system, user, assistant,
 * tool, and other (unaccounted overhead like formatting/framing tokens).
 *
 * This is a rough heuristic based on character count of message parts.
 * It will never be exact, but gives users a useful visual sense of what
 * is consuming their context window.
 *
 * @param messages - All messages in the session
 * @param parts - Map of messageId -> Part[]
 * @param inputTokens - Actual input token count from the last assistant message
 * @param systemPromptChars - Character length of the system prompt (if known)
 */
export function estimateContextBreakdown(
	messages: Message[],
	parts: Record<string, Part[] | undefined>,
	inputTokens: number,
	systemPromptChars?: number,
): ContextBreakdownSegment[] {
	if (!inputTokens || inputTokens <= 0) return []

	// Accumulate character counts per role
	const counts = { system: systemPromptChars ?? 0, user: 0, assistant: 0, tool: 0 }

	for (const msg of messages) {
		const msgParts = parts[msg.id]
		if (!msgParts) continue

		if (msg.role === "user") {
			for (const part of msgParts) {
				if (part.type === "text") counts.user += part.text.length
				else if (part.type === "file" && part.source && "text" in part.source) {
					counts.user += part.source.text.value.length
				} else if (part.type === "agent" && part.source) {
					counts.user += part.source.value.length
				}
			}
		} else if (msg.role === "assistant") {
			for (const part of msgParts) {
				if (part.type === "text") {
					counts.assistant += part.text.length
				} else if (part.type === "reasoning") {
					counts.assistant += part.text.length
				} else if (part.type === "tool") {
					const inputLen = Object.keys(part.state.input).length * 16
					if (part.state.status === "completed") {
						counts.tool += inputLen + part.state.output.length
					} else if (part.state.status === "error") {
						counts.tool += inputLen + part.state.error.length
					} else if (part.state.status === "pending") {
						counts.tool += inputLen + part.state.raw.length
					} else {
						counts.tool += inputLen
					}
				}
			}
		}
	}

	// Convert char counts to estimated tokens
	const tokens = {
		system: estimateTokens(counts.system),
		user: estimateTokens(counts.user),
		assistant: estimateTokens(counts.assistant),
		tool: estimateTokens(counts.tool),
	}
	const estimated = tokens.system + tokens.user + tokens.assistant + tokens.tool

	// Scale to match the actual input token count, or distribute "other" for the gap
	const build = (t: {
		system: number
		user: number
		assistant: number
		tool: number
		other: number
	}): ContextBreakdownSegment[] => {
		const toPercent = (v: number) => (v / inputTokens) * 100
		const toPercentLabel = (v: number) => Math.round(toPercent(v) * 10) / 10

		return (
			[
				{ key: "system" as const, tokens: t.system },
				{ key: "user" as const, tokens: t.user },
				{ key: "assistant" as const, tokens: t.assistant },
				{ key: "tool" as const, tokens: t.tool },
				{ key: "other" as const, tokens: t.other },
			] satisfies { key: ContextBreakdownKey; tokens: number }[]
		)
			.filter((x) => x.tokens > 0)
			.map((x) => ({
				key: x.key,
				tokens: x.tokens,
				width: toPercent(x.tokens),
				percent: toPercentLabel(x.tokens),
			}))
	}

	if (estimated <= inputTokens) {
		return build({ ...tokens, other: inputTokens - estimated })
	}

	// Estimated exceeds actual: scale everything down proportionally
	const scale = inputTokens / estimated
	const scaled = {
		system: Math.floor(tokens.system * scale),
		user: Math.floor(tokens.user * scale),
		assistant: Math.floor(tokens.assistant * scale),
		tool: Math.floor(tokens.tool * scale),
	}
	const total = scaled.system + scaled.user + scaled.assistant + scaled.tool
	return build({ ...scaled, other: Math.max(0, inputTokens - total) })
}

// ============================================================
// Formatters
// ============================================================

/** Format milliseconds as a compact duration string: "1s", "12s", "1m 34s", "2h 5m". */
export function formatWorkDuration(ms: number): string {
	if (ms <= 0) return "0s"
	const seconds = Math.ceil(ms / 1000)
	if (seconds < 60) return `${seconds}s`
	const minutes = Math.floor(seconds / 60)
	const remainingSeconds = seconds % 60
	if (minutes < 60) {
		return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`
	}
	const hours = Math.floor(minutes / 60)
	const remainingMinutes = minutes % 60
	return remainingMinutes > 0 ? `${hours}h ${remainingMinutes}m` : `${hours}h`
}

/** Format a USD cost value: "$0.00", "$0.12", "$1.23". */
export function formatCost(cost: number): string {
	if (cost < 0.005) return "$0.00"
	return `$${cost.toFixed(2)}`
}

/** Format a token count with compact notation: "0", "1.2k", "45.3k", "1.2M". */
export function formatTokens(count: number): string {
	if (count < 1000) return `${Math.round(count)}`
	if (count < 1_000_000) {
		const k = count / 1000
		return k >= 10 ? `${Math.round(k)}k` : `${k.toFixed(1)}k`
	}
	const m = count / 1_000_000
	return m >= 10 ? `${Math.round(m)}M` : `${m.toFixed(1)}M`
}

/** Format a percentage: "0%", "42%", "99.5%". */
export function formatPercentage(pct: number): string {
	if (pct < 0.5) return "0%"
	if (pct >= 99.5) return "100%"
	return pct >= 10 ? `${Math.round(pct)}%` : `${pct.toFixed(1)}%`
}

/**
 * Shorten a model ID for compact display.
 * "claude-sonnet-4-20250514" -> "sonnet-4"
 * "gpt-4o-2024-08-06" -> "gpt-4o"
 * "o3-mini" -> "o3-mini"
 * Falls back to the full ID if no known pattern matches.
 */
export function shortModelName(modelID: string): string {
	if (!modelID) return ""

	// Claude models: claude-{variant}-{version}-{date}
	const claudeMatch = modelID.match(/^claude-(.+?)(-\d{8})?$/)
	if (claudeMatch) return claudeMatch[1]

	// GPT models: gpt-{variant}-{date}
	const gptMatch = modelID.match(/^(gpt-\w+?)(-\d{4}-\d{2}-\d{2})?$/)
	if (gptMatch) return gptMatch[1]

	// Gemini models: gemini-{variant}-{date}
	const geminiMatch = modelID.match(/^(gemini-[\w.-]+?)(-\d{4}-?\d{2})?$/)
	if (geminiMatch) return geminiMatch[1]

	// Generic: strip trailing date patterns (YYYYMMDD or YYYY-MM-DD)
	const genericMatch = modelID.match(/^(.+?)(-\d{8}|-\d{4}-\d{2}-\d{2})$/)
	if (genericMatch) return genericMatch[1]

	return modelID
}
