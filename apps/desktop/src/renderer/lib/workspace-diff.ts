import type {
	WorkspaceChangedFile,
	WorkspaceChangedFileStatus,
	WorkspaceChangeView,
} from "@devo-ai/sdk/v2/client"

export type ReviewFileStatus = "added" | "deleted" | "modified"

export type WorkspacePatchFile = {
	file: string
	status: ReviewFileStatus
	rawStatus: WorkspaceChangedFileStatus
	additions: number
	deletions: number
	binary: boolean
	diffTruncated: boolean
	patch: string | null
	/** True while Summary list is shown and Full patch has not arrived yet. */
	patchPending: boolean
	/** Full previous-side text for MultiFileDiff; null when unavailable. */
	oldText: string | null
	/** Full new-side text for MultiFileDiff; null when unavailable. */
	newText: string | null
	warnings: string[]
}

/** Normalize path separators so Windows PathBuf keys match git patch paths. */
export function normalizeWorkspacePath(path: string): string {
	return path.replace(/\\/g, "/")
}

/** Header-only stubs (no hunks) are not real patches — keep lazy-loading. */
export function isCompletePatch(patch: string | null | undefined): boolean {
	if (!patch) return false
	return (
		/^@@ /m.test(patch) ||
		/Binary files /.test(patch) ||
		/GIT binary patch/.test(patch)
	)
}

export function numberFromProtocol(value: unknown): number {
	if (typeof value === "number" && Number.isFinite(value)) return value
	if (typeof value === "bigint") return Number(value)
	if (typeof value === "string") {
		const parsed = Number(value)
		if (Number.isFinite(parsed)) return parsed
	}
	return 0
}

export function workspaceChangeStats(view: WorkspaceChangeView | null | undefined): {
	fileCount: number
	additions: number
	deletions: number
} {
	if (!view) return { fileCount: 0, additions: 0, deletions: 0 }
	return {
		fileCount: numberFromProtocol(view.stats.files_changed),
		additions: numberFromProtocol(view.stats.additions),
		deletions: numberFromProtocol(view.stats.deletions),
	}
}

export function workspacePatchFilesFromView(
	view: WorkspaceChangeView | null | undefined,
): WorkspacePatchFile[] {
	if (!view) return []
	const patches = patchesByPath(view.unified_diff ?? "")
	return view.files.map((file) => {
		const path = normalizeWorkspacePath(String(file.path))
		const binary = Boolean(file.binary)
		const patch = patches.get(path) ?? null
		const complete = isCompletePatch(patch)
		return {
			file: path,
			status: reviewStatus(file.status),
			rawStatus: file.status,
			additions: numberFromProtocol(file.additions),
			deletions: numberFromProtocol(file.deletions),
			binary,
			diffTruncated: Boolean(file.diff_truncated),
			patch: complete ? patch : null,
			// Missing or header-only stub → still waiting on path-scoped Full.
			patchPending: !binary && !complete && file.old_text == null && file.new_text == null,
			oldText: typeof file.old_text === "string" ? file.old_text : null,
			newText: typeof file.new_text === "string" ? file.new_text : null,
			warnings: warningsForFile(view, file),
		}
	})
}

function warningsForFile(
	_view: WorkspaceChangeView,
	file: WorkspaceChangedFile,
): string[] {
	const warnings: string[] = []
	if (file.binary) warnings.push("Binary file")
	if (file.diff_truncated) warnings.push("Diff truncated")
	// Missing unified_diff is normal for Summary responses (patches upgrade later).
	return warnings
}

function reviewStatus(status: WorkspaceChangedFileStatus): ReviewFileStatus {
	switch (status) {
		case "added":
		case "untracked":
			return "added"
		case "deleted":
			return "deleted"
		case "modified":
		case "renamed":
		case "type_changed":
		case "unknown":
			return "modified"
	}
}

function patchesByPath(diff: string): Map<string, string> {
	const map = new Map<string, string>()
	for (const chunk of splitGitDiff(diff)) {
		const path = pathFromPatch(chunk)
		if (!path) continue
		map.set(path, chunk.endsWith("\n") ? chunk : `${chunk}\n`)
	}
	return map
}

function splitGitDiff(diff: string): string[] {
	if (!diff.trim()) return []
	const lines = diff.split(/(?=^diff --git )/m)
	return lines.map((line) => line.trimStart()).filter(Boolean)
}

function pathFromPatch(patch: string): string | null {
	const header = patch.match(/^diff --git a\/(.+?) b\/(.+)$/m)
	if (header) return cleanPath(header[2])
	const renamed = patch.match(/^\+\+\+ b\/(.+)$/m)
	if (renamed) return cleanPath(renamed[1])
	const deleted = patch.match(/^--- a\/(.+)$/m)
	if (deleted) return cleanPath(deleted[1])
	return null
}

function cleanPath(path: string): string {
	return normalizeWorkspacePath(path.replace(/^"|"$/g, ""))
}
