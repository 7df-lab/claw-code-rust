/**
 * Settings tab for environment setup, migration management, and re-running onboarding.
 */

import { Button } from "@devo/ui/components/button"
import { Spinner } from "@devo/ui/components/spinner"
import { useAtomValue, useSetAtom } from "jotai"
import {
	AlertCircleIcon,
	CheckCircle2Icon,
	RefreshCwIcon,
	RotateCcwIcon,
	UndoIcon,
} from "lucide-react"
import { useCallback, useEffect, useState } from "react"
import type { DevoCheckResult } from "../../../preload/api"
import { onboardingStateAtom } from "../../atoms/onboarding"
import { SettingsHeader } from "./settings-header"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

const isElectron = typeof window !== "undefined" && "devo" in window

// ============================================================
// Provider display metadata
// ============================================================

const PROVIDER_LABELS: Record<string, string> = {
	"claude-code": "Claude Code",
	cursor: "Cursor",
	devo: "Devo",
	opencode: "OpenCode",
}

export function SetupSettings() {
	return (
		<div className="space-y-10">
			<SettingsHeader title="Setup" />

			<DevoStatusSection />
			<MigrationSection />
			<OnboardingSection />
		</div>
	)
}

// ============================================================
// Devo runtime status
// ============================================================

function DevoStatusSection() {
	const [checking, setChecking] = useState(false)
	const [result, setResult] = useState<DevoCheckResult | null>(null)

	const checkStatus = useCallback(async () => {
		if (!isElectron) return
		setChecking(true)
		try {
			const r = await window.devo.onboarding.checkDevo()
			setResult(r)
		} catch {
			// ignore
		} finally {
			setChecking(false)
		}
	}, [])

	useEffect(() => {
		checkStatus()
	}, [checkStatus])

	return (
		<SettingsSection title="Desktop Runtime">
			<SettingsRow label="Version" description={result?.path ?? "Checking..."}>
				<div className="flex items-center gap-2">
					{checking ? (
						<Spinner className="size-3.5" />
					) : result?.installed ? (
						<>
							<span className="text-sm text-muted-foreground">
								{result.version && /^\d+\.\d+/.test(result.version)
									? `v${result.version}`
									: result.version}
							</span>
							{result.compatible ? (
								<CheckCircle2Icon className="size-4 text-emerald-500" />
							) : (
								<AlertCircleIcon className="size-4 text-amber-500" />
							)}
						</>
					) : (
						<span className="text-sm text-red-500">Not found</span>
					)}
					<Button
						variant="outline"
						size="sm"
						onClick={checkStatus}
						disabled={checking}
						className="gap-1.5"
					>
						<RefreshCwIcon aria-hidden="true" className="size-3" />
						Check
					</Button>
				</div>
			</SettingsRow>

			{result && !result.compatible && result.message && (
				<div className="px-5 py-2.5 text-[13px] text-amber-500">{result.message}</div>
			)}
		</SettingsSection>
	)
}

// ============================================================
// Migration management
// ============================================================

function MigrationSection() {
	const onboardingState = useAtomValue(onboardingStateAtom)
	const [restoring, setRestoring] = useState(false)
	const [restoreResult, setRestoreResult] = useState<string | null>(null)

	const handleRestore = useCallback(async () => {
		if (!isElectron) return
		setRestoring(true)
		setRestoreResult(null)
		try {
			const result = await window.devo.onboarding.restoreBackup()
			if (result.success) {
				setRestoreResult(`Restored ${result.restored.length} file(s)`)
			} else {
				setRestoreResult(`Errors: ${result.errors.join(", ")}`)
			}
		} catch (err) {
			setRestoreResult(err instanceof Error ? err.message : "Restore failed")
		} finally {
			setRestoring(false)
		}
	}, [])

	const migratedFrom = onboardingState.migratedFrom ?? []

	if (!onboardingState.migrationPerformed || migratedFrom.length === 0) {
		return null
	}

	const migratedLabels = migratedFrom.map((p) => PROVIDER_LABELS[p] ?? p).join(", ")

	return (
		<SettingsSection title="Configuration Migration">
			<SettingsRow label="Migrated from" description={migratedLabels}>
				<CheckCircle2Icon className="size-4 text-emerald-500" />
			</SettingsRow>
			<SettingsRow
				label="Last migrated"
				description={
					onboardingState.completedAt
						? new Date(onboardingState.completedAt).toLocaleString()
						: "Unknown"
				}
			>
				<span className="text-xs text-muted-foreground">
					{migratedFrom.length} provider{migratedFrom.length === 1 ? "" : "s"}
				</span>
			</SettingsRow>
			<SettingsRow
				label="Restore backup"
				description="Undo the migration and restore original files"
			>
				<div className="flex items-center gap-2">
					{restoreResult && <span className="text-xs text-muted-foreground">{restoreResult}</span>}
					<Button
						variant="outline"
						size="sm"
						onClick={handleRestore}
						disabled={restoring}
						className="gap-1.5"
					>
						{restoring ? (
							<Spinner className="size-3" />
						) : (
							<UndoIcon aria-hidden="true" className="size-3" />
						)}
						Restore
					</Button>
				</div>
			</SettingsRow>
		</SettingsSection>
	)
}

// ============================================================
// Re-run onboarding
// ============================================================

function OnboardingSection() {
	const setOnboardingState = useSetAtom(onboardingStateAtom)

	const handleRerun = useCallback(() => {
		setOnboardingState({
			completed: false,
			completedAt: null,
			skippedSteps: [],
			migrationPerformed: false,
			migratedFrom: [],
			devoVersion: null,
			providersConnected: 0,
		})
		// Relaunch the app to show onboarding fresh
		if (isElectron) {
			window.devo.relaunch()
		}
	}, [setOnboardingState])

	return (
		<SettingsSection title="Onboarding">
			<SettingsRow
				label="Re-run setup"
				description="Reset and show the onboarding wizard again on next launch"
			>
				<Button variant="outline" size="sm" onClick={handleRerun} className="gap-1.5">
					<RotateCcwIcon aria-hidden="true" className="size-3" />
					Re-run Setup
				</Button>
			</SettingsRow>
		</SettingsSection>
	)
}
