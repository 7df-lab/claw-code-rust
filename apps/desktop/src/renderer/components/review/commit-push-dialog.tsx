/**
 * CommitPushDialog -- lightweight "commit everything and push" dialog for the
 * ReviewPanel. Unlike the worktree CommitDialog there is no branch selection
 * or PR step: `git add -A` + commit + push on the session's directory, then a
 * refresh callback so the panel re-reads the (now clean) working tree.
 */

import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Textarea } from "@devo/ui/components/textarea"
import { GitCommitHorizontalIcon, Loader2Icon } from "lucide-react"
import { useCallback, useEffect, useState } from "react"
import type { GitDiffStat } from "../../../preload/api"
import { fetchDiffStat, gitCommitAll, gitPush } from "../../services/backend"

interface CommitPushDialogProps {
	open: boolean
	onOpenChange: (open: boolean) => void
	directory: string
	/** Called after a successful commit (even if the push failed) */
	onCommitted: () => void
}

export function CommitPushDialog({
	open,
	onOpenChange,
	directory,
	onCommitted,
}: CommitPushDialogProps) {
	const [diffStat, setDiffStat] = useState<GitDiffStat | null>(null)
	const [loadingDiff, setLoadingDiff] = useState(false)
	const [commitMessage, setCommitMessage] = useState("")
	const [executing, setExecuting] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const [success, setSuccess] = useState<string | null>(null)

	// Load diff stat when dialog opens
	useEffect(() => {
		if (!open) return
		setDiffStat(null)
		setCommitMessage("")
		setError(null)
		setSuccess(null)
		setLoadingDiff(true)
		fetchDiffStat(directory)
			.then(setDiffStat)
			.catch(() => setDiffStat(null))
			.finally(() => setLoadingDiff(false))
	}, [open, directory])

	const filesChanged = diffStat?.filesChanged ?? 0
	const nothingToCommit = !loadingDiff && !!diffStat && filesChanged === 0

	const handleExecute = useCallback(async () => {
		setExecuting(true)
		setError(null)
		try {
			const msg =
				commitMessage.trim() ||
				`Update ${filesChanged} file${filesChanged !== 1 ? "s" : ""}`
			const commitResult = await gitCommitAll(directory, msg)
			if (!commitResult.success) {
				setError(`Commit failed: ${commitResult.error}`)
				return
			}
			// The tree is clean now regardless of whether the push works.
			onCommitted()
			const pushResult = await gitPush(directory)
			if (!pushResult.success) {
				setError(`Committed, but push failed: ${pushResult.error}`)
				return
			}
			setSuccess("Committed and pushed")
			setTimeout(() => onOpenChange(false), 1200)
		} catch (err) {
			setError(err instanceof Error ? err.message : "Operation failed")
		} finally {
			setExecuting(false)
		}
	}, [commitMessage, directory, filesChanged, onCommitted, onOpenChange])

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="max-w-md">
				<DialogHeader>
					<DialogTitle className="flex items-center gap-2">
						<GitCommitHorizontalIcon className="size-5" />
						Commit &amp; push
					</DialogTitle>
					<DialogDescription>
						Stage all changes, commit, and push the current branch.
					</DialogDescription>
				</DialogHeader>

				<div className="space-y-4">
					<div className="rounded-md bg-muted px-3 py-2 text-sm">
						{loadingDiff ? (
							<span className="flex items-center gap-2 text-muted-foreground">
								<Loader2Icon className="size-3.5 animate-spin" />
								Scanning changes...
							</span>
						) : nothingToCommit ? (
							<span className="text-muted-foreground">No changes detected</span>
						) : (
							<span>
								{filesChanged} file{filesChanged !== 1 ? "s" : ""} changed
							</span>
						)}
					</div>

					<Textarea
						value={commitMessage}
						onChange={(e) => setCommitMessage(e.target.value)}
						placeholder="Commit message (optional)"
						className="min-h-[60px] resize-none text-sm"
					/>

					{error && (
						<div className="rounded-md bg-destructive/10 px-3 py-2 text-sm break-words text-destructive">
							{error}
						</div>
					)}
					{success && (
						<div className="rounded-md bg-green-500/10 px-3 py-2 text-sm text-green-600">
							{success}
						</div>
					)}
				</div>

				<DialogFooter>
					<Button variant="outline" onClick={() => onOpenChange(false)} disabled={executing}>
						Cancel
					</Button>
					<Button onClick={handleExecute} disabled={executing || loadingDiff || nothingToCommit}>
						{executing && <Loader2Icon className="size-3.5 animate-spin" />}
						Commit &amp; push
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}
