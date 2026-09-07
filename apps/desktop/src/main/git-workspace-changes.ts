import simpleGit from "simple-git"

/**
 * Local (Electron main) workspace change reads.
 *
 * Working-tree Summaries and per-file patches go through IPC + simple-git so
 * Changes never waits on stdio RPC for a whole-tree Full diff. Turn Summaries
 * still come from the server (checkpoint metadata); expand uses local
 * `git diff <checkpoint> -- <path>` when a checkpoint id is available.
 */

export type LocalGitChangeScope = "uncommitted" | "staged" | "unstaged" | "branch"

export type LocalChangedFileStatus =
	| "added"
	| "deleted"
	| "modified"
	| "renamed"
	| "untracked"
	| "type_changed"
	| "unknown"

export type LocalChangedFile = {
	path: string
	status: LocalChangedFileStatus
	additions: number | null
	deletions: number | null
	binary: boolean
	diff_truncated: boolean
}

export type LocalWorkspaceChangeView = {
	scope: LocalGitChangeScope
	status: "ready" | "empty" | "unsupported" | "partial" | "error"
	workspace_root: string
	coverage: "git_visible" | "partial"
	attribution: "git_working_tree" | "git_branch"
	change_set_status: "accumulating" | "finalized"
	files: LocalChangedFile[]
	stats: { files_changed: number; additions: number; deletions: number }
	unified_diff: string | null
	warnings: string[]
	generated_at: string
}

const MAX_UNTRACKED = 500

function getGit(directory: string) {
	return simpleGit({ baseDir: directory, trimmed: true })
}

function parseStatus(code: string): LocalChangedFileStatus {
	switch (code[0]) {
		case "A":
			return "added"
		case "D":
			return "deleted"
		case "M":
		case "T":
			return "modified"
		case "R":
		case "C":
			return "renamed"
		case "?":
			return "untracked"
		default:
			return "unknown"
	}
}

function parseNameStatus(output: string): Map<string, LocalChangedFileStatus> {
	const map = new Map<string, LocalChangedFileStatus>()
	for (const line of output.split("\n")) {
		const trimmed = line.trimEnd()
		if (!trimmed) continue
		const parts = trimmed.split("\t")
		if (parts.length < 2) continue
		const status = parseStatus(parts[0] ?? "")
		const path = parts.length >= 3 ? (parts[2] ?? parts[1]) : parts[1]
		if (path) map.set(path.replace(/\\/g, "/"), status)
	}
	return map
}

function parseNumstat(output: string): Map<string, { additions: number; deletions: number; binary: boolean }> {
	const map = new Map<string, { additions: number; deletions: number; binary: boolean }>()
	for (const line of output.split("\n")) {
		const trimmed = line.trimEnd()
		if (!trimmed) continue
		const parts = trimmed.split("\t")
		if (parts.length < 3) continue
		const addRaw = parts[0] ?? "0"
		const delRaw = parts[1] ?? "0"
		const path = parts.length >= 4 ? (parts[3] ?? parts[2]) : parts[2]
		if (!path) continue
		const binary = addRaw === "-" || delRaw === "-"
		map.set(path.replace(/\\/g, "/"), {
			additions: binary ? 0 : Number(addRaw) || 0,
			deletions: binary ? 0 : Number(delRaw) || 0,
			binary,
		})
	}
	return map
}

async function rangeArgs(
	directory: string,
	scope: LocalGitChangeScope,
	baseBranch?: string | null,
): Promise<{ args: string[]; includeUntracked: boolean; attribution: LocalWorkspaceChangeView["attribution"] } | null> {
	const git = getGit(directory)
	try {
		await git.raw(["rev-parse", "--verify", "HEAD"])
	} catch {
		return null
	}

	switch (scope) {
		case "uncommitted":
			return { args: ["HEAD", "--"], includeUntracked: true, attribution: "git_working_tree" }
		case "staged":
			return {
				args: ["--cached", "HEAD", "--"],
				includeUntracked: false,
				attribution: "git_working_tree",
			}
		case "unstaged":
			return { args: ["--"], includeUntracked: true, attribution: "git_working_tree" }
		case "branch": {
			const branch = baseBranch?.trim() || "main"
			let mergeBase = ""
			try {
				mergeBase = (await git.raw(["merge-base", branch, "HEAD"])).trim()
			} catch {
				try {
					mergeBase = (await git.raw(["merge-base", `origin/${branch}`, "HEAD"])).trim()
				} catch {
					return null
				}
			}
			if (!mergeBase) return null
			return {
				args: [mergeBase, "HEAD", "--"],
				includeUntracked: false,
				attribution: "git_branch",
			}
		}
	}
}

/**
 * Fast file list for a git scope (name-status + numstat). No unified patch.
 */
export async function localWorkspaceChangesSummary(
	directory: string,
	scope: LocalGitChangeScope,
	options: { baseBranch?: string | null; ignoreWhitespace?: boolean } = {},
): Promise<LocalWorkspaceChangeView> {
	const generated_at = new Date().toISOString()
	const ranged = await rangeArgs(directory, scope, options.baseBranch)
	if (!ranged) {
		return {
			scope,
			status: "unsupported",
			workspace_root: directory,
			coverage: "partial",
			attribution: scope === "branch" ? "git_branch" : "git_working_tree",
			change_set_status: scope === "branch" ? "finalized" : "accumulating",
			files: [],
			stats: { files_changed: 0, additions: 0, deletions: 0 },
			unified_diff: null,
			warnings: ["not_git_or_no_head"],
			generated_at,
		}
	}

	const git = getGit(directory)
	const ws = options.ignoreWhitespace ? ["--ignore-all-space"] : []
	const [nameStatus, numstat] = await Promise.all([
		git.raw(["diff", "--name-status", "--no-textconv", "--no-ext-diff", ...ws, ...ranged.args]),
		git.raw(["diff", "--numstat", "--no-textconv", "--no-ext-diff", ...ws, ...ranged.args]),
	])

	const statuses = parseNameStatus(nameStatus)
	const statsMap = parseNumstat(numstat)
	const files: LocalChangedFile[] = []
	const warnings: string[] = []

	for (const [path, status] of statuses) {
		const num = statsMap.get(path)
		files.push({
			path,
			status,
			additions: num?.additions ?? null,
			deletions: num?.deletions ?? null,
			binary: num?.binary ?? false,
			diff_truncated: false,
		})
	}

	if (ranged.includeUntracked) {
		const untrackedRaw = await git.raw(["ls-files", "--others", "--exclude-standard"])
		const untracked = untrackedRaw
			.split("\n")
			.map((line) => line.trim())
			.filter(Boolean)
			.map((path) => path.replace(/\\/g, "/"))
		const total = untracked.length
		for (const path of untracked.slice(0, MAX_UNTRACKED)) {
			if (files.some((file) => file.path === path)) continue
			files.push({
				path,
				status: "untracked",
				additions: null,
				deletions: null,
				binary: false,
				diff_truncated: false,
			})
		}
		if (total > MAX_UNTRACKED) {
			warnings.push(`untracked_files_truncated: showing ${MAX_UNTRACKED} of ${total}`)
		}
	}

	let additions = 0
	let deletions = 0
	for (const file of files) {
		additions += file.additions ?? 0
		deletions += file.deletions ?? 0
	}

	return {
		scope,
		status: files.length === 0 ? "empty" : warnings.length > 0 ? "partial" : "ready",
		workspace_root: directory,
		coverage: warnings.length > 0 ? "partial" : "git_visible",
		attribution: ranged.attribution,
		change_set_status: scope === "branch" ? "finalized" : "accumulating",
		files,
		stats: { files_changed: files.length, additions, deletions },
		unified_diff: null,
		warnings,
		generated_at,
	}
}

/**
 * Single-file unified diff for expand-on-demand. Avoids whole-tree Full over stdio.
 *
 * Pass `fileStatus` from the Summary row to skip an extra `ls-files` probe.
 * Pass `checkpointId` for turn-scope diffs against a ghost/checkpoint commit.
 * Pass `mergeBase` for branch scope to skip a merge-base lookup per expand.
 */
export async function localWorkspaceFilePatch(
	directory: string,
	scope: LocalGitChangeScope | "turn",
	filePath: string,
	options: {
		baseBranch?: string | null
		ignoreWhitespace?: boolean
		fileStatus?: LocalChangedFileStatus | string | null
		checkpointId?: string | null
		mergeBase?: string | null
	} = {},
): Promise<string> {
	const normalized = filePath.replace(/\\/g, "/")
	const git = getGit(directory)
	const ws = options.ignoreWhitespace ? ["--ignore-all-space"] : []
	const statusHint = options.fileStatus ?? null

	if (statusHint === "untracked") {
		return buildUntrackedPatch(directory, normalized)
	}

	if (scope === "turn") {
		const checkpoint = options.checkpointId?.trim()
		if (!checkpoint) return ""
		return (
			await git.raw([
				"diff",
				"--no-textconv",
				"--no-ext-diff",
				...ws,
				checkpoint,
				"--",
				normalized,
			])
		).trimEnd()
	}

	// Working-tree scopes: do not call rangeArgs (avoids merge-base on every click).
	if (scope === "uncommitted" || scope === "unstaged") {
		if (statusHint == null) {
			try {
				const others = await git.raw([
					"ls-files",
					"--others",
					"--exclude-standard",
					"--",
					normalized,
				])
				if (others.trim().length > 0) {
					return buildUntrackedPatch(directory, normalized)
				}
			} catch {
				// fall through
			}
		}
		const args =
			scope === "unstaged"
				? ["diff", "--no-textconv", "--no-ext-diff", ...ws, "--", normalized]
				: ["diff", "--no-textconv", "--no-ext-diff", ...ws, "HEAD", "--", normalized]
		return (await git.raw(args)).trimEnd()
	}

	if (scope === "staged") {
		return (
			await git.raw([
				"diff",
				"--no-textconv",
				"--no-ext-diff",
				...ws,
				"--cached",
				"HEAD",
				"--",
				normalized,
			])
		).trimEnd()
	}

	// branch — match Summary (`merge-base..HEAD`), not working-tree-only.
	let mergeBase = options.mergeBase?.trim() || null
	if (!mergeBase) {
		const ranged = await rangeArgs(directory, "branch", options.baseBranch)
		if (!ranged) return ""
		mergeBase = ranged.args[0] ?? null
	}
	if (!mergeBase) return ""
	return (
		await git.raw([
			"diff",
			"--no-textconv",
			"--no-ext-diff",
			...ws,
			mergeBase,
			"HEAD",
			"--",
			normalized,
		])
	).trimEnd()
}

async function buildUntrackedPatch(directory: string, normalized: string): Promise<string> {
	const fs = await import("node:fs/promises")
	const path = await import("node:path")
	const abs = path.join(directory, normalized)
	let content = ""
	try {
		content = await fs.readFile(abs, "utf8")
	} catch {
		return `diff --git a/${normalized} b/${normalized}\nnew file mode 100644\n--- /dev/null\n+++ b/${normalized}\n`
	}
	// Cap huge untracked files so expand never freezes the UI.
	const maxChars = 400_000
	const truncated = content.length > maxChars
	const slice = truncated ? content.slice(0, maxChars) : content
	const lines = slice.split(/\r?\n/)
	const body = lines.map((line) => `+${line}`).join("\n")
	const count = lines.length
	const parts = [
		`diff --git a/${normalized} b/${normalized}`,
		"new file mode 100644",
		"--- /dev/null",
		`+++ b/${normalized}`,
		`@@ -0,0 +1,${count} @@`,
		body,
	]
	if (truncated) parts.push("+/* truncated */")
	return `${parts.join("\n")}\n`
}
