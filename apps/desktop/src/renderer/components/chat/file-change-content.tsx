/**
 * Expandable body for write / edit / apply_patch tool rows:
 * Cursor-style unified diff via @pierre/diffs (Diff / DiffContent).
 */

import { Diff, DiffContent } from "@devo/ui/components/ai-elements/diff"
import { AlertTriangleIcon, MinusIcon, PlusIcon } from "lucide-react"
import type { ReactNode } from "react"
import type { ToolPart } from "../../lib/types"
import {
	fileChangeEntries,
	fileChangeUnifiedDiff,
	looksLikeUnifiedDiff,
	type FileChangeEntryView,
	type FileChangeStats,
} from "./file-change-presentation"

/** Green +N / red −M inline after the file path (omit zero sides). */
export function FileChangeStatsBadge({ stats }: { stats: FileChangeStats }) {
	if (stats.additions === 0 && stats.deletions === 0) return null
	return (
		<span className="inline-flex shrink-0 items-center gap-1.5 text-[11px] align-middle">
			{stats.additions > 0 && (
				<span className="inline-flex items-center gap-0.5 text-diff-addition-foreground">
					<PlusIcon className="size-2.5" aria-hidden="true" />
					{stats.additions}
				</span>
			)}
			{stats.deletions > 0 && (
				<span className="inline-flex items-center gap-0.5 text-diff-deletion-foreground">
					<MinusIcon className="size-2.5" aria-hidden="true" />
					{stats.deletions}
				</span>
			)}
		</span>
	)
}

const MAX_OUTPUT_LENGTH = 5000

function truncateOutput(output: string, max = MAX_OUTPUT_LENGTH): string {
	if (output.length <= max) return output
	return `${output.slice(0, max)}\n... (truncated)`
}

function fileNameFromPath(path: string | undefined): string {
	if (!path) return "file"
	const normalized = path.replace(/\\/g, "/")
	const segments = normalized.split("/")
	return segments[segments.length - 1] || "file"
}

function ErrorBlock({ error }: { error: string }) {
	return (
		<div className="mx-3.5 my-2.5 flex items-start gap-2 rounded bg-muted/30 px-2 py-1.5 text-xs text-muted-foreground">
			<AlertTriangleIcon className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
			<pre className="max-h-32 overflow-auto font-mono">
				<code>{error.length > 500 ? `${error.slice(0, 500)}...` : error}</code>
			</pre>
		</div>
	)
}

function DiffShell({ children }: { children: ReactNode }) {
	return (
		<div className="max-h-96 overflow-hidden text-[11px] [&_.diff]:border-0">{children}</div>
	)
}

function FilesDiff({
	path,
	oldContent,
	newContent,
}: {
	path?: string
	oldContent: string
	newContent: string
}) {
	const name = fileNameFromPath(path)
	return (
		<Diff
			mode="files"
			oldFile={{ name, content: oldContent }}
			newFile={{ name, content: newContent }}
			className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
		>
			<DiffContent maxHeight={384} showLineNumbers hideFileHeader diffStyle="unified" />
		</Diff>
	)
}

function PatchDiffView({ patch }: { patch: string }) {
	return (
		<Diff
			mode="patch"
			patch={patch}
			className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
		>
			<DiffContent maxHeight={384} showLineNumbers hideFileHeader diffStyle="unified" />
		</Diff>
	)
}

function renderEntry(entry: FileChangeEntryView, key: string): ReactNode {
	if (entry.oldString != null && entry.newString != null) {
		return (
			<FilesDiff
				key={key}
				path={entry.path}
				oldContent={entry.oldString}
				newContent={entry.newString}
			/>
		)
	}
	if (entry.unifiedDiff) {
		return <PatchDiffView key={key} patch={entry.unifiedDiff} />
	}
	if (entry.content != null) {
		// Add → empty old file (all green); Delete → empty new file (all red).
		const isDelete = entry.changeType === "delete"
		return (
			<FilesDiff
				key={key}
				path={entry.path}
				oldContent={isDelete ? entry.content : ""}
				newContent={isDelete ? "" : entry.content}
			/>
		)
	}
	return null
}

/**
 * Inline unified-diff body for write / edit / apply_patch.
 * Prefer old/new strings, then unifiedDiff, then Add/Delete content as a files diff.
 */
export function FileChangeContent({ part }: { part: ToolPart }) {
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined
	if (error) return <ErrorBlock error={error} />

	const input = part.state.input as Record<string, unknown> | undefined
	const entries = fileChangeEntries(input)
	const rendered = entries
		.map((entry, index) => renderEntry(entry, `${entry.path ?? "file"}-${index}`))
		.filter(Boolean)

	if (rendered.length > 0) {
		return rendered.length === 1 ? (
			<>{rendered[0]}</>
		) : (
			<DiffShell>
				<div className="space-y-3">{rendered}</div>
			</DiffShell>
		)
	}

	// Fallback: completed output that looks like a patch (legacy / apply_patch).
	const output = part.state.status === "completed" ? part.state.output : undefined
	if (output && looksLikeUnifiedDiff(output)) {
		return <PatchDiffView patch={output} />
	}

	const unifiedFromInput = fileChangeUnifiedDiff(input)
	if (unifiedFromInput) {
		return <PatchDiffView patch={unifiedFromInput} />
	}

	if (output) {
		return (
			<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
				<code>{truncateOutput(output)}</code>
			</pre>
		)
	}

	return null
}
