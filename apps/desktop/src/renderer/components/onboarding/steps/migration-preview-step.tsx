/**
 * Migration Preview & Execute step.
 *
 * Shows a file tree of what will be created/modified, a diff preview of
 * selected files, and executes the migration with backup on confirmation.
 * Supports all migration providers (Claude Code, Cursor, Devo, OpenCode).
 */

import { Button } from "@devo/ui/components/button"
import { Spinner } from "@devo/ui/components/spinner"
import {
	AlertTriangleIcon,
	ArrowLeftIcon,
	FileIcon,
	FolderIcon,
	FolderOpenIcon,
	PlayIcon,
} from "lucide-react"
import { useCallback, useEffect, useState } from "react"
import type {
	MigrationPreview,
	MigrationProgress,
	MigrationProvider,
	MigrationResult,
} from "../../../../preload/api"

// ============================================================
// Types
// ============================================================

interface MigrationPreviewStepProps {
	provider: MigrationProvider
	scanResult: unknown
	categories: string[]
	preview: MigrationPreview | null
	onComplete: (result: MigrationResult) => void
	onBack: () => void
	onSkip: () => void
}

// ============================================================
// Provider display metadata
// ============================================================

const PROVIDER_LABELS: Record<MigrationProvider, string> = {
	"claude-code": "Claude Code",
	cursor: "Cursor",
	devo: "Devo",
	opencode: "OpenCode",
}

// ============================================================
// Component
// ============================================================

export function MigrationPreviewStep({
	provider,
	scanResult,
	categories,
	preview,
	onComplete,
	onBack,
	onSkip,
}: MigrationPreviewStepProps) {
	const [selectedFile, setSelectedFile] = useState<string | null>(null)
	const [executing, setExecuting] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const [progress, setProgress] = useState<MigrationProgress | null>(null)

	const isElectron = typeof window !== "undefined" && "devo" in window
	const label = PROVIDER_LABELS[provider]

	// Subscribe to migration progress events during execution
	useEffect(() => {
		if (!isElectron || !executing) return
		const unsub = window.devo.onboarding.onMigrationProgress((p) => {
			setProgress(p as MigrationProgress)
		})
		return unsub
	}, [isElectron, executing])

	const handleExecute = useCallback(async () => {
		if (!isElectron || !scanResult) return
		setExecuting(true)
		setError(null)
		setProgress(null)

		try {
			const result = await window.devo.onboarding.executeMigration(
				provider,
				scanResult,
				categories,
			)
			onComplete(result)
		} catch (err) {
			setError(err instanceof Error ? err.message : "Migration failed")
			setExecuting(false)
		}
	}, [isElectron, provider, scanResult, categories, onComplete])

	if (!preview) return null

	// Find the selected file's content for the diff preview
	const selectedFileContent = (() => {
		for (const cat of preview.categories) {
			for (const file of cat.files) {
				if (file.path === selectedFile) return file.content
			}
		}
		return null
	})()

	return (
		<div className="flex h-full flex-col px-6 py-4">
			<div className="mx-auto w-full max-w-3xl space-y-4">
				{/* Header */}
				<div className="text-center">
					<h2 className="text-[22px] font-medium tracking-tight text-foreground">{label} Migration Preview</h2>
					<p className="mt-1 text-sm text-muted-foreground">
						{preview.fileCount} file(s) will be created. Review the changes below.
					</p>
				</div>

				{/* Session import summary */}
				{preview.sessionCount > 0 && (
					<div
						data-slot="onboarding-card"
						className="flex items-center gap-3 rounded-lg border border-border bg-muted/20 p-3"
					>
						<FolderOpenIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
						<p className="text-sm text-muted-foreground">
							{preview.sessionCount} session{preview.sessionCount === 1 ? "" : "s"} across{" "}
							{preview.sessionProjectCount} project
							{preview.sessionProjectCount === 1 ? "" : "s"} will be imported.
						</p>
					</div>
				)}

				{/* File tree + preview split */}
				<div className="flex gap-4" style={{ minHeight: "300px", maxHeight: "400px" }}>
					{/* File tree */}
					<div
						data-slot="onboarding-card"
						className="w-1/2 overflow-y-auto rounded-lg border border-border bg-background p-2"
					>
						{preview.categories.map((cat) => (
							<div key={cat.category} className="mb-2">
								<div className="flex items-center gap-2 px-2 py-1 text-xs font-medium uppercase tracking-wider text-muted-foreground">
									<FolderIcon aria-hidden="true" className="size-3" />
									{cat.category} ({cat.itemCount})
								</div>
								{cat.files.map((file) => (
									<button
										key={file.path}
										type="button"
										onClick={() => setSelectedFile(file.path)}
										className={`flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs transition-colors ${
											selectedFile === file.path
												? "bg-muted text-foreground"
												: "text-muted-foreground hover:bg-muted/50 hover:text-foreground"
										}`}
									>
										<FileIcon aria-hidden="true" className="size-3 shrink-0" />
										<span className="min-w-0 truncate">{shortenPath(file.path)}</span>
										<span
											className={`ml-auto shrink-0 rounded px-1 py-0.5 text-[10px] font-medium uppercase ${
												file.status === "new"
													? "bg-emerald-500/10 text-emerald-500"
													: file.status === "modified"
														? "bg-amber-500/10 text-amber-500"
														: "bg-muted text-muted-foreground"
											}`}
										>
											{file.status}
										</span>
									</button>
								))}
							</div>
						))}
					</div>

					{/* File preview */}
					<div
						data-slot="onboarding-card"
						className="w-1/2 overflow-y-auto rounded-lg border border-border bg-zinc-950 p-3 font-mono text-xs text-zinc-300"
					>
						{selectedFileContent ? (
							<pre className="whitespace-pre-wrap break-all">{selectedFileContent}</pre>
						) : (
							<div className="flex h-full items-center justify-center text-zinc-500">
								Select a file to preview
							</div>
						)}
					</div>
				</div>

				{/* Warnings */}
				{preview.warnings.length > 0 && (
					<div className="space-y-1 rounded-lg border border-amber-500/20 bg-amber-500/5 p-3">
						<div className="flex items-center gap-2 text-xs font-medium text-amber-500">
							<AlertTriangleIcon aria-hidden="true" className="size-3.5" />
							Warnings
						</div>
						{preview.warnings.map((w) => (
							<p key={w} className="text-xs text-amber-500/80">
								{w}
							</p>
						))}
					</div>
				)}

				{/* Manual actions */}
				{preview.manualActions.length > 0 && (
					<div className="space-y-1 rounded-lg border border-border bg-muted/20 p-3">
						<div className="text-xs font-medium text-muted-foreground">
							Needs manual attention after migration:
						</div>
						{preview.manualActions.map((a) => (
							<p key={a} className="text-xs text-muted-foreground">
								- {a}
							</p>
						))}
					</div>
				)}

				{/* Error */}
				{error && (
					<div className="rounded-md border border-red-500/20 bg-red-500/10 px-3 py-2 text-sm text-red-500">
						{error}
					</div>
				)}

				{/* Backup notice */}
				<p className="text-center text-xs text-muted-foreground/60">
					A backup will be saved to ~/.config/devo/backups/ before any changes.
				</p>

				{/* Actions */}
				<div className="flex items-center justify-center gap-3">
					<Button variant="outline" onClick={onBack} className="gap-2">
						<ArrowLeftIcon aria-hidden="true" className="size-3.5" />
						Back
					</Button>
					<Button variant="outline" onClick={onSkip}>
						Skip
					</Button>
					<Button onClick={handleExecute} disabled={executing} className="gap-2">
						{executing ? (
							<>
								<Spinner className="size-3.5" />
								{formatProgressLabel(progress)}
							</>
						) : (
							<>
								<PlayIcon aria-hidden="true" className="size-3.5" />
								Apply Migration
							</>
						)}
					</Button>
				</div>
			</div>
		</div>
	)
}

// ============================================================
// Helpers
// ============================================================

/** Format migration progress into a short label for the button. */
function formatProgressLabel(progress: MigrationProgress | null): string {
	if (!progress) return "Migrating..."

	switch (progress.phase) {
		case "converting":
			return "Converting sessions..."
		case "dedup-check":
			return "Checking for duplicates..."
		case "writing":
			if (progress.total > 0) {
				return `Writing session ${progress.current}/${progress.total}...`
			}
			return "Writing sessions..."
		case "complete":
			return "Finishing..."
		default:
			return "Migrating..."
	}
}

/** Shorten a file path for display by replacing the home directory with ~. */
function shortenPath(filePath: string): string {
	// Try to shorten common prefixes
	const homePatterns = ["/Users/", "/home/", "C:\\Users\\"]
	for (const pattern of homePatterns) {
		const idx = filePath.indexOf(pattern)
		if (idx !== -1) {
			const afterHome = filePath.slice(idx + pattern.length)
			const slashIdx =
				afterHome.indexOf("/") !== -1 ? afterHome.indexOf("/") : afterHome.indexOf("\\")
			if (slashIdx !== -1) {
				return `~${afterHome.slice(slashIdx)}`
			}
		}
	}
	return filePath
}
