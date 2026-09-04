/**
 * Provider constants and metadata shared across onboarding, settings, and dialogs.
 */

// ============================================================
// Provider ordering
// ============================================================

/** Popular providers shown prominently in onboarding and settings, in display order.
 * Ordered to match the built-in catalog (crates/core/providers.json). */
export const POPULAR_PROVIDER_IDS = [
	"deepseek",
	"kimi",
	"qwen",
	"openai",
	"zai",
	"zhipu",
	"ollama",
	"minimax",
	"tencent",
	"xiaomi",
	"poolside",
] as const

// ============================================================
// Devo Zen
// ============================================================

/** The provider ID for Devo Zen (always auto-loads, free tier available) */
export const ZEN_PROVIDER_ID = "devo"

/** URL to sign up for an Devo Zen API key */
export const ZEN_SIGNUP_URL = "https://devo.ai/zen/"

/** URL to Devo Zen documentation */
export const ZEN_DOCS_URL = "https://devo.ai/docs/zen"

// ============================================================
// Provider key signup URLs
// ============================================================

/** Providers that can connect without an API key (local / no-auth). */
export const PROVIDERS_OPTIONAL_API_KEY = new Set<string>(["ollama"])

/** External URLs for getting API keys from popular providers */
export const PROVIDER_KEY_URLS: Record<string, { label: string; url: string }> = {
	devo: { label: "Get API key", url: "https://devo.ai/zen/" },
	anthropic: { label: "Get API key", url: "https://console.anthropic.com/settings/keys" },
	openai: { label: "Get API key", url: "https://platform.openai.com/api-keys" },
	google: { label: "Get API key", url: "https://aistudio.google.com/apikey" },
	groq: { label: "Get API key", url: "https://console.groq.com/keys" },
	openrouter: { label: "Get API key", url: "https://openrouter.ai/keys" },
	xai: { label: "Get API key", url: "https://console.x.ai/" },
	mistral: { label: "Get API key", url: "https://console.mistral.ai/api-keys/" },
	deepseek: { label: "Get API key", url: "https://platform.deepseek.com/api_keys" },
	cohere: { label: "Get API key", url: "https://dashboard.cohere.com/api-keys" },
	fireworks: { label: "Get API key", url: "https://fireworks.ai/account/api-keys" },
	perplexity: { label: "Get API key", url: "https://www.perplexity.ai/settings/api" },
}

// ============================================================
// Sorting helpers
// ============================================================

/** Sort comparator: popular providers first (by POPULAR_PROVIDER_IDS order), then alphabetical */
export function compareByPopularity(
	a: { id: string; name: string },
	b: { id: string; name: string },
): number {
	const aIdx = POPULAR_PROVIDER_IDS.indexOf(a.id as (typeof POPULAR_PROVIDER_IDS)[number])
	const bIdx = POPULAR_PROVIDER_IDS.indexOf(b.id as (typeof POPULAR_PROVIDER_IDS)[number])
	if (aIdx !== -1 && bIdx !== -1) return aIdx - bIdx
	if (aIdx !== -1) return -1
	if (bIdx !== -1) return 1
	return a.name.localeCompare(b.name)
}

/** Sort comparator: connected first, then by popularity */
export function compareConnectedFirst(
	connectedIds: Set<string>,
	a: { id: string; name: string },
	b: { id: string; name: string },
): number {
	const aConnected = connectedIds.has(a.id)
	const bConnected = connectedIds.has(b.id)
	if (aConnected && !bConnected) return -1
	if (!aConnected && bConnected) return 1
	return compareByPopularity(a, b)
}

/**
 * Model usable context capacity: `contextWindow × percent / 100` (default 95%).
 *
 * The Models editor shows this absolute token count. On save it keeps the hard
 * `contextWindow` and stores the choice as `effectiveContextWindowPercent`.
 */
export function effectiveContextWindowTokens(model: {
	contextWindow?: number | null
	effectiveContextWindowPercent?: number | null
}): number | undefined {
	if (model.contextWindow == null || !Number.isFinite(model.contextWindow)) {
		return undefined
	}
	const percent = model.effectiveContextWindowPercent ?? 95
	return Math.floor((model.contextWindow * percent) / 100)
}

/** Map a user-entered absolute usable window onto percent of hard capacity. */
export function contextWindowPercentFromAbsolute(
	hardWindow: number,
	absoluteTokens: number,
): number {
	const hard = Math.max(1, Math.floor(hardWindow))
	const absolute = Math.max(0, Math.floor(absoluteTokens))
	const percent = (absolute * 100) / hard
	return Math.min(100, Math.max(1, percent))
}

export function formatContextWindowLabel(tokens: number | undefined): string | null {
	if (tokens == null || !Number.isFinite(tokens) || tokens <= 0) return null
	return `${Math.round(tokens / 1000)}k`
}

// ============================================================
// Zen tier detection
// ============================================================

/**
 * Threshold for distinguishing Zen free tier from paid.
 * The server strips paid models when there is no API key, leaving only ~6 free models.
 * With an API key, 20+ models are available.
 */
const ZEN_FREE_TIER_MAX_MODELS = 10

/**
 * Heuristic: determine whether the Zen provider is on the free tier
 * by checking how many models the server returned. The server strips
 * paid models for users without an API key, so a low model count
 * means free tier.
 */
export function isZenFreeTier(models: Record<string, unknown>): boolean {
	return Object.keys(models).length <= ZEN_FREE_TIER_MAX_MODELS
}

// ============================================================
// Subscription detection
// ============================================================

interface ModelWithCost {
	cost?: { input?: number }
}

/**
 * Heuristic: detect whether a provider is connected via a subscription plan
 * (e.g. Claude Pro/Max) by checking if ALL model costs are zeroed out.
 *
 * The Devo auth plugins zero out costs for subscription-based OAuth
 * connections (Anthropic Pro/Max, OpenAI ChatGPT Pro/Plus). The `source`
 * field from the server is unreliable for this distinction, so we use
 * model costs instead.
 */
export function isSubscriptionConnected(models: Record<string, unknown>): boolean {
	const entries = Object.values(models) as ModelWithCost[]
	if (entries.length === 0) return false
	return entries.every((m) => m.cost?.input === 0)
}

/** Display labels for subscription-connected providers */
export const SUBSCRIPTION_LABELS: Record<string, string> = {
	anthropic: "Claude Pro/Max",
	openai: "ChatGPT Pro/Plus",
}
