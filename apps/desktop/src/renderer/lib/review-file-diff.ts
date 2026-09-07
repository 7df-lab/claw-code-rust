/**
 * Build an expandable FileDiff from git's unified patch + full file sides.
 *
 * MultiFileDiff re-diffs via jsdiff and can diverge from git numstat (especially
 * LF/CRLF). Feeding the git patch into processFile with both sides keeps hunks
 * aligned with git while isPartial=false so expand-up/down still works.
 */

import { processFile, type FileDiffMetadata } from "@pierre/diffs"

export type ExpandableGitFileDiffInput = {
	patch: string
	fileName: string
	/** Previous-side text; null becomes empty string for add/untracked. */
	oldText: string | null
	/** New-side text; null becomes empty string for delete. */
	newText: string | null
}

export function buildExpandableGitFileDiff(
	input: ExpandableGitFileDiffInput,
): FileDiffMetadata | null {
	const patch = input.patch.trim()
	if (!patch) return null
	try {
		const fileDiff = processFile(patch, {
			isGitDiff: patch.includes("diff --git"),
			oldFile: { name: input.fileName, contents: input.oldText ?? "" },
			newFile: { name: input.fileName, contents: input.newText ?? "" },
			throwOnError: false,
		})
		if (fileDiff == null || fileDiff.isPartial) return null
		return fileDiff
	} catch {
		return null
	}
}

/** Count addition + deletion lines across hunks (excludes context). */
export function countChangedLinesInFileDiff(fileDiff: FileDiffMetadata): {
	additions: number
	deletions: number
} {
	let additions = 0
	let deletions = 0
	for (const hunk of fileDiff.hunks) {
		additions += hunk.additionLines
		deletions += hunk.deletionLines
	}
	return { additions, deletions }
}
