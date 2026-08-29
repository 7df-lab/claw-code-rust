/**
 * Context-window occupancy (what fills the model window), matching
 * Native `ContextOccupancy` and the TUI `/status` category breakdown.
 */

export const CONTEXT_CATEGORY_IDS = [
	"base",
	"skills",
	"toolsBuiltin",
	"toolsMcp",
	"conversation",
] as const

export type ContextCategoryId = (typeof CONTEXT_CATEGORY_IDS)[number]

export interface ContextCategoryUsage {
	id: ContextCategoryId
	tokens: number
	/** Share of occupancy in basis points (0..=10_000). */
	shareBps: number
}

export interface ContextOccupancy {
	totalTokens: number
	contextWindowTokens: number
	categories: ContextCategoryUsage[]
}

export interface ContextCategoryRow {
	id: ContextCategoryId
	label: string
	tokens: number
	shareBps: number
	/** Occupancy share as a 0–100 integer, matching TUI `/status`. */
	sharePercent: number
}

const CATEGORY_LABELS: Record<ContextCategoryId, string> = {
	base: "Base",
	skills: "Skills",
	toolsBuiltin: "Tools (builtin)",
	toolsMcp: "Tools (MCP)",
	conversation: "Conversation",
}

const CATEGORY_IDS = new Set<string>(CONTEXT_CATEGORY_IDS)

export function isContextCategoryId(value: string): value is ContextCategoryId {
	return CATEGORY_IDS.has(value)
}

export function contextCategoryLabel(id: ContextCategoryId): string {
	return CATEGORY_LABELS[id]
}

/** Window fill 0–100 from occupancy tokens vs effective window. */
export function occupancyWindowPercent(occupancy: ContextOccupancy | null | undefined): number {
	if (!occupancy) return 0
	return windowFillPercent(occupancy.totalTokens, occupancy.contextWindowTokens)
}

export function windowFillPercent(used: number, window: number): number {
	if (window <= 0) return 0
	return Math.max(0, Math.min(100, Math.round((used / window) * 100)))
}

/**
 * Stable TUI `/status` category order, filling missing buckets with zeros.
 */
export function occupancyCategoryRows(
	occupancy: ContextOccupancy | null | undefined,
): ContextCategoryRow[] {
	const byId = new Map<ContextCategoryId, ContextCategoryUsage>()
	for (const category of occupancy?.categories ?? []) {
		if (!isContextCategoryId(category.id)) continue
		byId.set(category.id, category)
	}
	return CONTEXT_CATEGORY_IDS.map((id) => {
		const category = byId.get(id)
		const tokens = category?.tokens ?? 0
		const shareBps = category?.shareBps ?? 0
		return {
			id,
			label: CATEGORY_LABELS[id],
			tokens,
			shareBps,
			sharePercent: Math.floor(shareBps / 100),
		}
	})
}
