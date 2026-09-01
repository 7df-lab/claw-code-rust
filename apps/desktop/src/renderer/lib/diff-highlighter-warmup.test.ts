import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const warmupSource = readFileSync(
	new URL("../lib/diff-highlighter-warmup.ts", import.meta.url),
	"utf8",
)

describe("diff-highlighter-warmup", () => {
	test("preloads pierre highlighter themes used by inline diffs", () => {
		expect({
			exportsWarmup: warmupSource.includes("export function warmDiffHighlighter"),
			preloadsThemes: warmupSource.includes("preloadHighlighter"),
			includesOneDark: warmupSource.includes("one-dark-pro"),
		}).toEqual({
			exportsWarmup: true,
			preloadsThemes: true,
			includesOneDark: true,
		})
	})
})
