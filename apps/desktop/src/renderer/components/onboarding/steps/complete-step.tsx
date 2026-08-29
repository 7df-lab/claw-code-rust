/**
 * Onboarding: Complete / Ready.
 *
 * Shows a success state and quick tips. Provider config import is not offered
 * from first-run onboarding.
 */

import { Button } from "@devo/ui/components/button"
import { CheckCircle2Icon, CommandIcon } from "lucide-react"
import { motion } from "motion/react"

// ============================================================
// Types
// ============================================================

interface CompleteStepProps {
	devoVersion: string | null
	onFinish: () => void
}

// ============================================================
// Component
// ============================================================

const isElectron = typeof window !== "undefined" && "devo" in window
const isMac = isElectron && window.devo.platform === "darwin"

export function CompleteStep({ devoVersion, onFinish }: CompleteStepProps) {
	const modKey = isMac ? "Cmd" : "Ctrl"

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
							? `Devo is connected to Devo ${formatVersion(devoVersion)}.`
							: "Devo is ready to go."}
					</p>
				</motion.div>

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
