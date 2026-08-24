/**
 * Onboarding: Complete / Ready.
 *
 * Shows a success state, quick tips, and optional prompts to migrate
 * from detected providers (Claude Code, Cursor, Devo, OpenCode). Migration
 * cards only appear for providers that have config on disk and haven't
 * already been migrated.
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import { Spinner } from "@devo/ui/components/spinner"
import { ArrowRightIcon, CheckCircle2Icon, CommandIcon, FlaskConicalIcon } from "lucide-react"
import { motion } from "motion/react"
import { useEffect, useRef, useState } from "react"
import type { MigrationProvider, MigrationResult, ProviderDetection } from "../../../../preload/api"

// ============================================================
// Types
// ============================================================

interface CompleteStepProps {
	devoVersion: string | null
	migratedProviders: string[]
	migrationResult: MigrationResult | null
	onStartMigration: (provider: MigrationProvider) => void
	onFinish: () => void
}

// ============================================================
// Component
// ============================================================

const isElectron = typeof window !== "undefined" && "devo" in window
const isMac = isElectron && window.devo.platform === "darwin"

export function CompleteStep({
	devoVersion,
	migratedProviders,
	migrationResult,
	onStartMigration,
	onFinish,
}: CompleteStepProps) {
	const modKey = isMac ? "Cmd" : "Ctrl"

	// Detect available providers on mount
	const [providers, setProviders] = useState<ProviderDetection[]>([])
	const [detecting, setDetecting] = useState(false)
	const hasDetected = useRef(false)

	useEffect(() => {
		if (!isElectron || hasDetected.current) return
		hasDetected.current = true
		setDetecting(true)

		window.devo.onboarding
			.detectProviders()
			.then((detections) => {
				// Only show providers that were found and aren't Devo itself
				// (no point migrating Devo -> Devo)
				setProviders(detections.filter((d) => d.found && d.provider !== "devo"))
				setDetecting(false)
			})
			.catch(() => {
				setDetecting(false)
			})
	}, [])

	// Filter out already-migrated providers
	const availableProviders = providers.filter((p) => !migratedProviders.includes(p.provider))
	const hasMigrated = migratedProviders.length > 0

	return (
		<div className="flex h-full flex-col items-center justify-center px-6">
			<div className="w-full max-w-md space-y-8 text-center">
				{/* Animated checkmark */}
				<motion.div
					className="flex justify-center"
					initial={{ scale: 0, opacity: 0 }}
					animate={{ scale: 1, opacity: 1 }}
					transition={{
						type: "spring",
						stiffness: 260,
						damping: 20,
						delay: 0.1,
					}}
				>
					<div className="flex size-16 items-center justify-center rounded-full bg-emerald-500/10">
						<CheckCircle2Icon className="size-8 text-emerald-500" />
					</div>
				</motion.div>

				{/* Title */}
				<motion.div
					initial={{ opacity: 0, y: 8 }}
					animate={{ opacity: 1, y: 0 }}
					transition={{ delay: 0.3, duration: 0.3 }}
					className="space-y-2"
				>
					<h2 className="text-[28px] font-medium tracking-tight text-foreground">You're all set.</h2>
					<p className="text-sm text-muted-foreground">
						{devoVersion
							? `Devo is connected to Devo ${formatVersion(devoVersion)}`
							: "Devo is ready to go"}
						{hasMigrated ? " and your configuration has been migrated." : "."}
					</p>
				</motion.div>

				{/* Migration summary (shown after migration completes) */}
				{migrationResult && (
					<motion.div
						initial={{ opacity: 0, y: 8 }}
						animate={{ opacity: 1, y: 0 }}
						transition={{ delay: 0.45, duration: 0.3 }}
						data-slot="onboarding-card"
						className="rounded-lg border border-border bg-muted/20 p-3 text-left"
					>
						<div className="space-y-1 text-xs text-muted-foreground">
							{migrationResult.filesWritten.length > 0 && (
								<p>{migrationResult.filesWritten.length} file(s) created</p>
							)}
							{migrationResult.filesSkipped.length > 0 && (
								<p>{migrationResult.filesSkipped.length} file(s) skipped (already exist)</p>
							)}
							{migrationResult.historyDuplicatesSkipped > 0 && (
								<p>
									{migrationResult.historyDuplicatesSkipped} session(s) skipped (already imported)
								</p>
							)}
							{migrationResult.backupDir && <p>Backup saved</p>}
							{migrationResult.manualActions.length > 0 && (
								<p className="text-amber-500">
									{migrationResult.manualActions.length} item(s) need manual attention
								</p>
							)}
						</div>
					</motion.div>
				)}

				{/* Provider migration cards */}
				{detecting && (
					<motion.div
						initial={{ opacity: 0, y: 8 }}
						animate={{ opacity: 1, y: 0 }}
						transition={{ delay: 0.45, duration: 0.3 }}
						className="flex items-center justify-center gap-2 text-sm text-muted-foreground"
					>
						<Spinner className="size-3.5" />
						Checking for existing configurations...
					</motion.div>
				)}

				{!detecting && availableProviders.length > 0 && (
					<motion.div
						initial={{ opacity: 0, y: 8 }}
						animate={{ opacity: 1, y: 0 }}
						transition={{ delay: 0.45, duration: 0.3 }}
						className="space-y-2"
					>
						{availableProviders.map((provider) => (
							<button
								key={provider.provider}
								type="button"
								onClick={() => onStartMigration(provider.provider)}
								data-slot="onboarding-card"
								className="group w-full cursor-pointer rounded-lg border border-border bg-muted/20 p-4 text-left transition-colors hover:bg-muted/40"
							>
								<div className="flex items-center justify-between">
									<div className="space-y-1">
										<p className="flex items-center gap-2 text-sm font-medium text-foreground">
											Migrate from {provider.label}?
											<Badge
												variant="outline"
												className="gap-1 px-1.5 py-0 text-[10px] text-muted-foreground"
											>
												<FlaskConicalIcon aria-hidden="true" className="size-2.5" />
												Experimental
											</Badge>
										</p>
										<p className="text-xs text-muted-foreground">{provider.summary}</p>
									</div>
									<ArrowRightIcon
										aria-hidden="true"
										className="size-4 shrink-0 text-muted-foreground transition-transform group-hover:translate-x-0.5"
									/>
								</div>
							</button>
						))}
					</motion.div>
				)}

				{/* Quick tips */}
				<motion.div
					initial={{ opacity: 0, y: 8 }}
					animate={{ opacity: 1, y: 0 }}
					transition={{ delay: 0.55, duration: 0.3 }}
					className="space-y-2"
				>
					<p className="text-xs font-medium uppercase tracking-wider text-muted-foreground/50">
						Quick tips
					</p>
					<div className="flex justify-center">
						<div className="space-y-1.5 text-left text-sm text-muted-foreground">
							<ShortcutRow keys={[modKey, "K"]} label="Command palette" />
							<ShortcutRow keys={[modKey, "N"]} label="New session" />
							<ShortcutRow keys={[modKey, ","]} label="Settings" />
						</div>
					</div>
				</motion.div>

				{/* CTA */}
				<motion.div
					initial={{ opacity: 0, y: 8 }}
					animate={{ opacity: 1, y: 0 }}
					transition={{ delay: 0.7, duration: 0.3 }}
					className="flex items-center justify-center gap-3"
				>
					<Button size="lg" onClick={onFinish}>
						Start Building
					</Button>
				</motion.div>
			</div>
		</div>
	)
}

// ============================================================
// Helpers
// ============================================================

/** Format a version string for display. Semver gets a "v" prefix, non-semver gets parens. */
function formatVersion(version: string): string {
	if (/^\d+\.\d+/.test(version)) return `v${version}`
	return `(${version})`
}

// ============================================================
// Sub-components
// ============================================================

function ShortcutRow({ keys, label }: { keys: string[]; label: string }) {
	return (
		<div className="flex items-center gap-3">
			<div className="flex items-center gap-0.5">
				{keys.map((key) => (
					<kbd
						key={key}
						className="inline-flex h-5 min-w-[20px] items-center justify-center rounded border border-border bg-muted px-1 font-mono text-[10px] font-medium text-muted-foreground"
					>
						{key === "Cmd" ? <CommandIcon aria-hidden="true" className="size-2.5" /> : key}
					</kbd>
				))}
			</div>
			<span>{label}</span>
		</div>
	)
}
