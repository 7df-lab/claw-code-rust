import { describe, expect, test } from "bun:test"
import type { WorkspaceChangeView } from "@devo-ai/sdk/v2/client"
import { workspacePatchFilesFromView } from "./workspace-diff"

describe("workspacePatchFilesFromView", () => {
	test("splits unified git diff into per-file patch entries", () => {
		const view = {
			scope: "turn",
			status: "ready",
			workspace_root: "/repo",
			coverage: "git_visible",
			attribution: "workspace_net",
			change_set_status: "finalized",
			files: [
				file("src/a.ts", "modified", 1, 1),
				file("src/b.ts", "added", 1, 0),
				file("src/c.ts", "deleted", 0, 1),
			],
			stats: { files_changed: 3, additions: 2, deletions: 2 },
			unified_diff: [
				"diff --git a/src/a.ts b/src/a.ts",
				"--- a/src/a.ts",
				"+++ b/src/a.ts",
				"@@ -1 +1 @@",
				"-old",
				"+new",
				"diff --git a/src/b.ts b/src/b.ts",
				"--- /dev/null",
				"+++ b/src/b.ts",
				"@@ -0,0 +1 @@",
				"+new",
				"diff --git a/src/c.ts b/src/c.ts",
				"--- a/src/c.ts",
				"+++ /dev/null",
				"@@ -1 +0,0 @@",
				"-old",
			].join("\n"),
			warnings: [],
			generated_at: "2026-06-26T00:00:00Z",
		} as unknown as WorkspaceChangeView

		expect(workspacePatchFilesFromView(view)).toEqual([
			expect.objectContaining({ file: "src/a.ts", patch: expect.stringContaining("-old") }),
			expect.objectContaining({ file: "src/b.ts", patch: expect.stringContaining("+new") }),
			expect.objectContaining({ file: "src/c.ts", patch: expect.stringContaining("-old") }),
		])
	})

	test("matches Windows PathBuf separators to git patch paths", () => {
		const view = {
			scope: "uncommitted",
			status: "ready",
			workspace_root: "C:\\repo",
			coverage: "git_visible",
			attribution: "git_working_tree",
			change_set_status: "accumulating",
			files: [file("apps\\desktop\\foo.ts", "modified", 1, 1)],
			stats: { files_changed: 1, additions: 1, deletions: 1 },
			unified_diff: [
				"diff --git a/apps/desktop/foo.ts b/apps/desktop/foo.ts",
				"--- a/apps/desktop/foo.ts",
				"+++ b/apps/desktop/foo.ts",
				"@@ -1 +1 @@",
				"-old",
				"+new",
			].join("\n"),
			warnings: [],
			generated_at: "2026-06-26T00:00:00Z",
		} as unknown as WorkspaceChangeView

		expect(workspacePatchFilesFromView(view)).toEqual([
			expect.objectContaining({
				file: "apps/desktop/foo.ts",
				patch: expect.stringContaining("+new"),
				patchPending: false,
			}),
		])
	})

	test("treats header-only untracked stubs as patchPending", () => {
		const view = {
			scope: "uncommitted",
			status: "ready",
			workspace_root: "/repo",
			coverage: "git_visible",
			attribution: "git_working_tree",
			change_set_status: "accumulating",
			files: [file("new.ts", "untracked", 0, 0)],
			stats: { files_changed: 1, additions: 0, deletions: 0 },
			unified_diff: [
				"diff --git a/new.ts b/new.ts",
				"new file mode 100644",
				"--- /dev/null",
				"+++ b/new.ts",
			].join("\n"),
			warnings: [],
			generated_at: "2026-06-26T00:00:00Z",
		} as unknown as WorkspaceChangeView

		expect(workspacePatchFilesFromView(view)[0]).toEqual(
			expect.objectContaining({
				file: "new.ts",
				status: "added",
				patch: null,
				patchPending: true,
			}),
		)
	})

	test("marks Summary-only rows as patchPending", () => {
		const view = {
			scope: "uncommitted",
			status: "ready",
			workspace_root: "/repo",
			coverage: "git_visible",
			attribution: "git_working_tree",
			change_set_status: "accumulating",
			files: [file("src/a.ts", "modified", 1, 1)],
			stats: { files_changed: 1, additions: 1, deletions: 1 },
			unified_diff: null,
			warnings: [],
			generated_at: "2026-06-26T00:00:00Z",
		} as unknown as WorkspaceChangeView

		expect(workspacePatchFilesFromView(view)[0]).toEqual(
			expect.objectContaining({
				file: "src/a.ts",
				patch: null,
				patchPending: true,
			}),
		)
	})

	test("keeps patchPending for files missing from a partial unified_diff", () => {
		const view = {
			scope: "uncommitted",
			status: "ready",
			workspace_root: "/repo",
			coverage: "git_visible",
			attribution: "git_working_tree",
			change_set_status: "accumulating",
			files: [file("src/a.ts", "modified", 1, 1), file("src/b.ts", "modified", 1, 0)],
			stats: { files_changed: 2, additions: 2, deletions: 1 },
			unified_diff: [
				"diff --git a/src/a.ts b/src/a.ts",
				"--- a/src/a.ts",
				"+++ b/src/a.ts",
				"@@ -1 +1 @@",
				"-old",
				"+new",
			].join("\n"),
			warnings: [],
			generated_at: "2026-06-26T00:00:00Z",
		} as unknown as WorkspaceChangeView

		const rows = workspacePatchFilesFromView(view)
		expect(rows[0]).toEqual(expect.objectContaining({ file: "src/a.ts", patchPending: false }))
		expect(rows[1]).toEqual(expect.objectContaining({ file: "src/b.ts", patch: null, patchPending: true }))
	})

	test("keeps metadata-only files visible", () => {
		const view = {
			scope: "turn",
			status: "partial",
			workspace_root: "/repo",
			coverage: "partial",
			attribution: "workspace_net",
			change_set_status: "finalized",
			files: [file("asset.bin", "modified", 0, 0, true, true)],
			stats: { files_changed: 1, additions: 0, deletions: 0 },
			warnings: ["large_file_without_text_diff"],
			generated_at: "2026-06-26T00:00:00Z",
		} as unknown as WorkspaceChangeView

		expect(workspacePatchFilesFromView(view)).toEqual([
			{
				file: "asset.bin",
				status: "modified",
				rawStatus: "modified",
				additions: 0,
				deletions: 0,
				binary: true,
				diffTruncated: true,
				patch: null,
				patchPending: false,
				oldText: null,
				newText: null,
				warnings: ["Binary file", "Diff truncated"],
			},
		])
	})

	test("copies old_text/new_text onto WorkspacePatchFile", () => {
		const view = {
			scope: "uncommitted",
			status: "ready",
			workspace_root: "/repo",
			coverage: "git_visible",
			attribution: "git_working_tree",
			change_set_status: "accumulating",
			files: [
				{
					...file("src/a.ts", "modified", 1, 1),
					old_text: "line1\nold\nline3\n",
					new_text: "line1\nnew\nline3\n",
				},
			],
			stats: { files_changed: 1, additions: 1, deletions: 1 },
			unified_diff: null,
			warnings: [],
			generated_at: "2026-06-26T00:00:00Z",
		} as unknown as WorkspaceChangeView

		expect(workspacePatchFilesFromView(view)[0]).toEqual(
			expect.objectContaining({
				file: "src/a.ts",
				patchPending: false,
				oldText: "line1\nold\nline3\n",
				newText: "line1\nnew\nline3\n",
			}),
		)
	})
})

function file(
	path: string,
	status: "added" | "modified" | "deleted" | "untracked",
	additions: number,
	deletions: number,
	binary = false,
	diffTruncated = false,
) {
	return {
		path,
		status,
		additions,
		deletions,
		binary,
		diff_truncated: diffTruncated,
	}
}
