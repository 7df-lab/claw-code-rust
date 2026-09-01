import { preloadHighlighter } from "@pierre/diffs"

const WARMUP_THEMES = ["one-dark-pro", "one-light"] as const
const WARMUP_LANGS = [
	"text",
	"typescript",
	"tsx",
	"javascript",
	"jsx",
	"rust",
	"json",
	"python",
	"shell",
] as const

let warmup: Promise<void> | null = null

/**
 * Preload the shared Shiki highlighter used by @pierre/diffs.
 * Pierre's first mount can render blank until the highlighter is ready; warming
 * on chat entry avoids the open → close → reopen workaround in inline diffs.
 */
export function warmDiffHighlighter(): Promise<void> {
	if (typeof window === "undefined") return Promise.resolve()
	if (warmup == null) {
		warmup = preloadHighlighter({
			themes: [...WARMUP_THEMES],
			langs: [...WARMUP_LANGS],
		}).catch((error: unknown) => {
			warmup = null
			throw error
		})
	}
	return warmup
}
