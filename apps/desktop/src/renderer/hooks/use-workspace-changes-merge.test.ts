import { describe, expect, test } from "bun:test"
import type { WorkspaceChangeView } from "@devo-ai/sdk/v2/client"
import {
	mergePathFullIntoView,
	mergeSummaryPreservingExpandState,
} from "./use-workspace-changes"

function baseView(overrides: Partial<WorkspaceChangeView> = {}): WorkspaceChangeView {
	return {
		scope: "uncommitted",
		status: "ready",
		workspace_root: "/repo",
		coverage: "git_visible",
		attribution: "git_working_tree",
		change_set_status: "accumulating",
		files: [
			{
				path: "src/a.ts",
				status: "modified",
				additions: 1n,
				deletions: 1n,
				binary: false,
				diff_truncated: false,
			},
		],
		stats: { files_changed: 1n, additions: 1n, deletions: 1n },
		warnings: [],
		generated_at: "2026-06-26T00:00:00Z",
		...overrides,
	} as WorkspaceChangeView
}

describe("mergePathFullIntoView", () => {
	test("merges old_text/new_text from path-scoped Full", () => {
		const base = baseView()
		const patch = baseView({
			files: [
				{
					path: "src/a.ts",
					status: "modified",
					additions: 1n,
					deletions: 1n,
					binary: false,
					diff_truncated: false,
					old_text: "old body\n",
					new_text: "new body\n",
				},
			],
			unified_diff: [
				"diff --git a/src/a.ts b/src/a.ts",
				"--- a/src/a.ts",
				"+++ b/src/a.ts",
				"@@ -1 +1 @@",
				"-old body",
				"+new body",
			].join("\n"),
		})

		const merged = mergePathFullIntoView(base, patch)
		expect(merged.files[0]).toEqual(
			expect.objectContaining({
				old_text: "old body\n",
				new_text: "new body\n",
			}),
		)
		expect(merged.unified_diff).toContain("+new body")
	})
})

describe("mergeSummaryPreservingExpandState", () => {
	test("keeps sides when Summary refreshes", () => {
		const previous = baseView({
			files: [
				{
					path: "src/a.ts",
					status: "modified",
					additions: 1n,
					deletions: 1n,
					binary: false,
					diff_truncated: false,
					old_text: "old\n",
					new_text: "new\n",
				},
			],
			unified_diff: "diff --git a/src/a.ts b/src/a.ts\n@@ -1 +1 @@\n-old\n+new\n",
		})
		const summary = baseView({
			files: [
				{
					path: "src/a.ts",
					status: "modified",
					additions: 2n,
					deletions: 2n,
					binary: false,
					diff_truncated: false,
				},
			],
			unified_diff: undefined,
		})

		const merged = mergeSummaryPreservingExpandState(previous, summary)
		expect(merged.files[0]).toEqual(
			expect.objectContaining({
				additions: 2n,
				old_text: "old\n",
				new_text: "new\n",
			}),
		)
		expect(merged.unified_diff).toContain("+new")
	})
})
