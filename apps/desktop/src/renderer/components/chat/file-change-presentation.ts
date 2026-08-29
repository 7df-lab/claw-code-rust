/**
 * Presentation helpers for write / edit / apply_patch tool rows:
 * Cursor-style verbs (Writing/Added, Editing/Edited), +/- line stats, and
 * the fields needed to render an expandable unified diff.
 */

export type FileChangeVerb = "Writing" | "Added" | "Editing" | "Edited"

export type FileChangeStats = {
	additions: number
	deletions: number
}

const FILE_CHANGE_TOOLS: ReadonlySet<string> = new Set(["write", "edit", "apply_patch"])

export function isFileChangeTool(tool: string): boolean {
	return FILE_CHANGE_TOOLS.has(tool)
}

export function fileChangePath(input: Record<string, unknown> | undefined): string | undefined {
	if (!input) return undefined
	const path = input.filePath ?? input.path
	return typeof path === "string" && path.trim() ? path : undefined
}

export function fileChangeContent(input: Record<string, unknown> | undefined): string | undefined {
	if (!input) return undefined
	return typeof input.content === "string" ? input.content : undefined
}

export function fileChangeOldNew(input: Record<string, unknown> | undefined): {
	oldString?: string
	newString?: string
} {
	if (!input) return {}
	return {
		oldString: typeof input.oldString === "string" ? input.oldString : undefined,
		newString: typeof input.newString === "string" ? input.newString : undefined,
	}
}

export function fileChangeUnifiedDiff(input: Record<string, unknown> | undefined): string | undefined {
	if (!input) return undefined
	if (typeof input.unifiedDiff === "string" && input.unifiedDiff) return input.unifiedDiff
	if (typeof input.unified_diff === "string" && input.unified_diff) return input.unified_diff
	if (typeof input.patch === "string" && looksLikeUnifiedDiff(input.patch)) return input.patch
	if (typeof input.diff === "string" && looksLikeUnifiedDiff(input.diff)) return input.diff
	return undefined
}

export function looksLikeUnifiedDiff(text: string): boolean {
	const trimmed = text.trimStart()
	return (
		trimmed.startsWith("---") ||
		trimmed.startsWith("diff --git") ||
		trimmed.startsWith("@@") ||
		trimmed.startsWith("*** Begin Patch")
	)
}

/**
 * Whether this file-change should use Added/Writing (create) vs Edited/Editing.
 * Prefer explicit Native changeType; otherwise write → add, edit/patch → update.
 */
export function isFileChangeAdd(tool: string, input: Record<string, unknown> | undefined): boolean {
	const changeType = typeof input?.changeType === "string" ? input.changeType : undefined
	if (changeType === "add") return true
	if (changeType === "update" || changeType === "delete") return false
	if (tool === "write") return true
	return false
}

export function fileChangeVerb(
	tool: string,
	options: { running?: boolean; input?: Record<string, unknown> },
): FileChangeVerb {
	const isAdd = isFileChangeAdd(tool, options.input)
	if (options.running) return isAdd ? "Writing" : "Editing"
	return isAdd ? "Added" : "Edited"
}

/** Count +/− lines from a unified diff, skipping file headers (+++ / ---). */
export function computeStatsFromUnifiedDiff(patch: string): FileChangeStats {
	let additions = 0
	let deletions = 0
	for (const line of patch.split("\n")) {
		if (line.startsWith("+") && !line.startsWith("+++")) additions++
		else if (line.startsWith("-") && !line.startsWith("---")) deletions++
	}
	return { additions, deletions }
}

/**
 * Rough line-set diff used when only old/new strings are available
 * (matches the previous desktop chat-tool-call behaviour).
 */
export function computeDiffStatsFromStrings(
	oldStr: string,
	newStr: string,
): FileChangeStats {
	const oldLines = oldStr.split("\n")
	const newLines = newStr.split("\n")
	const oldSet = new Set(oldLines)
	const newSet = new Set(newLines)

	let additions = 0
	let deletions = 0
	for (const line of newLines) {
		if (!oldSet.has(line)) additions++
	}
	for (const line of oldLines) {
		if (!newSet.has(line)) deletions++
	}
	return { additions, deletions }
}

function lineCount(content: string): number {
	if (content.length === 0) return 0
	// Match "N lines" semantics: trailing newline does not add an extra empty line.
	const lines = content.split("\n")
	return lines.length > 0 && lines[lines.length - 1] === "" ? lines.length - 1 : lines.length
}

/**
 * Resolve +/− stats for a file-change tool part.
 * Priority: unifiedDiff → Add content → old/new strings.
 */
export function fileChangeStats(
	tool: string,
	input: Record<string, unknown> | undefined,
): FileChangeStats | undefined {
	if (!isFileChangeTool(tool) || !input) return undefined

	const unifiedDiff = fileChangeUnifiedDiff(input)
	if (unifiedDiff) return computeStatsFromUnifiedDiff(unifiedDiff)

	const content = fileChangeContent(input)
	if (isFileChangeAdd(tool, input) && content != null) {
		return { additions: lineCount(content), deletions: 0 }
	}

	const { oldString, newString } = fileChangeOldNew(input)
	if (oldString != null && newString != null) {
		return computeDiffStatsFromStrings(oldString, newString)
	}

	// Delete with content only
	if (content != null && typeof input.changeType === "string" && input.changeType === "delete") {
		return { additions: 0, deletions: lineCount(content) }
	}

	return undefined
}

/** Multi-file apply_patch entries when SDK mapped `changes[]`. */
export type FileChangeEntryView = {
	path?: string
	changeType?: string
	content?: string
	unifiedDiff?: string
	oldString?: string
	newString?: string
}

export function fileChangeEntries(
	input: Record<string, unknown> | undefined,
): FileChangeEntryView[] {
	if (!input) return []
	if (Array.isArray(input.changes) && input.changes.length > 0) {
		return input.changes.map((entry) => {
			const record =
				entry && typeof entry === "object" ? (entry as Record<string, unknown>) : {}
			return {
				path: typeof record.path === "string" ? record.path : undefined,
				changeType: typeof record.changeType === "string" ? record.changeType : undefined,
				content: typeof record.content === "string" ? record.content : undefined,
				unifiedDiff:
					typeof record.unifiedDiff === "string"
						? record.unifiedDiff
						: typeof record.unified_diff === "string"
							? record.unified_diff
							: undefined,
				oldString: typeof record.oldString === "string" ? record.oldString : undefined,
				newString: typeof record.newString === "string" ? record.newString : undefined,
			}
		})
	}

	const path = fileChangePath(input)
	const content = fileChangeContent(input)
	const unifiedDiff = fileChangeUnifiedDiff(input)
	const { oldString, newString } = fileChangeOldNew(input)
	if (
		path == null &&
		content == null &&
		unifiedDiff == null &&
		oldString == null &&
		newString == null
	) {
		return []
	}
	return [
		{
			path,
			changeType: typeof input.changeType === "string" ? input.changeType : undefined,
			content,
			unifiedDiff,
			oldString,
			newString,
		},
	]
}

export function hasFileChangeExpandableContent(
	tool: string,
	input: Record<string, unknown> | undefined,
	output?: string,
): boolean {
	if (!isFileChangeTool(tool)) return false
	const entries = fileChangeEntries(input)
	for (const entry of entries) {
		if (entry.oldString != null && entry.newString != null) return true
		if (entry.unifiedDiff) return true
		if (entry.content) return true
	}
	if (output && looksLikeUnifiedDiff(output)) return true
	return false
}
