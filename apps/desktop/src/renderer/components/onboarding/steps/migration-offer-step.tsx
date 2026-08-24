/**
 * Multi-provider Migration Offer.
 *
 * Scans the selected provider's configuration and lets the user select which
 * categories to migrate to Devo format. The user explicitly opted in
 * (from the complete step), so scanning happens on mount.
 */

import { Button } from "@devo/ui/components/button"
import { Checkbox } from "@devo/ui/components/checkbox"
import { Spinner } from "@devo/ui/components/spinner"
import {
	ArrowRightIcon,
	BotIcon,
	CogIcon,
	FileTextIcon,
	FolderOpenIcon,
	PlugIcon,
	ScrollTextIcon,
	ServerIcon,
	ShieldIcon,
	TerminalIcon,
} from "lucide-react"
import { useCallback, useEffect, useRef, useState } from "react"
import type {
	MigrationPreview,
	MigrationProvider,
	ProviderDetection,
} from "../../../../preload/api"

// ============================================================
// Types
// ============================================================

interface MigrationCategory {
	id: string
	label: string
	description: string
	icon: typeof CogIcon
	count: number
	enabled: boolean
}

interface MigrationOfferStepProps {
	provider: MigrationProvider
	onPreview: (scanResult: unknown, categories: string[], preview: MigrationPreview) => void
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

export function MigrationOfferStep({ provider, onPreview, onSkip }: MigrationOfferStepProps) {
	const [categories, setCategories] = useState<MigrationCategory[]>([])
	const [scanning, setScanning] = useState(false)
	const [scanError, setScanError] = useState<string | null>(null)
	const [previewing, setPreviewing] = useState(false)
	const hasScanned = useRef(false)
	const scanResultRef = useRef<unknown>(null)

	const isElectron = typeof window !== "undefined" && "devo" in window
	const label = PROVIDER_LABELS[provider]

	// Run full scan on mount (user explicitly opted in)
	useEffect(() => {
		if (!isElectron || hasScanned.current) return
		hasScanned.current = true
		setScanning(true)

		window.devo.onboarding
			.scanProvider(provider)
			.then(({ detection, scanResult }) => {
				scanResultRef.current = scanResult
				setCategories(buildCategories(provider, detection))
				setScanning(false)
			})
			.catch((err) => {
				setScanError(err instanceof Error ? err.message : "Scan failed")
				setScanning(false)
			})
	}, [isElectron, provider])

	const toggleCategory = useCallback((id: string) => {
		setCategories((prev) => prev.map((c) => (c.id === id ? { ...c, enabled: !c.enabled } : c)))
	}, [])

	const handlePreview = useCallback(async () => {
		if (!isElectron || !scanResultRef.current) return
		setPreviewing(true)
		setScanError(null)

		const selectedIds = categories.filter((c) => c.enabled).map((c) => c.id)

		try {
			const preview = await window.devo.onboarding.previewMigration(
				provider,
				scanResultRef.current,
				selectedIds,
			)
			onPreview(scanResultRef.current, selectedIds, preview)
		} catch (err) {
			setScanError(err instanceof Error ? err.message : "Preview failed")
		} finally {
			setPreviewing(false)
		}
	}, [isElectron, provider, categories, onPreview])

	const enabledCount = categories.filter((c) => c.enabled).length

	return (
		<div className="flex h-full flex-col items-center justify-center px-6">
			<div className="w-full max-w-lg space-y-6">
				<div className="text-center">
					<h2 className="text-[22px] font-medium tracking-tight text-foreground">Migrate from {label}</h2>
					<p className="mt-1 text-sm text-muted-foreground">
						We detected an existing {label} setup. Devo can migrate your configuration to Devo
						format.
					</p>
				</div>

				{/* Loading state */}
				{scanning && (
					<div
						data-slot="onboarding-card"
						className="flex items-center justify-center gap-3 rounded-lg border border-border bg-muted/30 p-6"
					>
						<Spinner className="size-4" />
						<span className="text-sm text-muted-foreground">Scanning {label} configuration...</span>
					</div>
				)}

				{/* Category checkboxes */}
				{!scanning && categories.length > 0 && (
					<div className="space-y-2">
						{categories.map((cat) => {
							if (cat.count === 0) return null
							const Icon = cat.icon
							return (
								<button
									type="button"
									key={cat.id}
									data-slot="onboarding-card"
									onClick={() => toggleCategory(cat.id)}
									className="flex w-full cursor-pointer items-center gap-3 rounded-lg border border-border bg-background p-3 text-left transition-colors hover:bg-muted/30"
								>
									<Checkbox
										checked={cat.enabled}
										onCheckedChange={() => toggleCategory(cat.id)}
										aria-label={cat.label}
									/>
									<Icon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
									<div className="min-w-0 flex-1">
										<p className="text-sm font-medium text-foreground">{cat.label}</p>
										<p className="text-xs text-muted-foreground">{cat.description}</p>
									</div>
									<span className="shrink-0 text-xs tabular-nums text-muted-foreground">
										{cat.count}
									</span>
								</button>
							)
						})}
					</div>
				)}

				{/* Info about what migration does */}
				{!scanning && categories.length > 0 && (
					<div
						data-slot="onboarding-card"
						className="rounded-lg border border-border bg-muted/20 p-3"
					>
						<p className="text-xs leading-relaxed text-muted-foreground">
							{getMigrationDescription(provider)}
						</p>
					</div>
				)}

				{/* Error */}
				{scanError && (
					<div className="rounded-md border border-red-500/20 bg-red-500/10 px-3 py-2 text-sm text-red-500">
						{scanError}
					</div>
				)}

				{/* Actions */}
				<div className="flex items-center justify-center gap-3">
					<Button variant="outline" onClick={onSkip}>
						Back
					</Button>
					{!scanning && categories.length > 0 && (
						<Button
							onClick={handlePreview}
							disabled={enabledCount === 0 || previewing}
							className="gap-2"
						>
							{previewing ? (
								<>
									<Spinner className="size-3.5" />
									Preparing preview...
								</>
							) : (
								<>
									Preview Changes
									<ArrowRightIcon aria-hidden="true" className="size-4" />
								</>
							)}
						</Button>
					)}
				</div>
			</div>
		</div>
	)
}

// ============================================================
// Helpers
// ============================================================

function getMigrationDescription(provider: MigrationProvider): string {
	switch (provider) {
		case "claude-code":
			return "Model IDs are translated automatically. MCP servers are converted to Devo format. Agent frontmatter is adapted. A backup is created before any changes, and you can undo at any time from Settings."
		case "cursor":
			return "MCP servers, rules (.mdc), agents, and commands are converted to Devo format. Cursor-specific features like OAuth and rule modes are adapted where possible. A backup is created before any changes."
		case "devo":
			return "Configuration, agents, commands, and rules are imported. A backup is created before any changes, and you can undo at any time from Settings."
		case "opencode":
			return "OpenCode providers and model bindings are imported through Devo provider settings. API keys are stored through Devo's provider credential flow."
	}
}

function buildCategories(
	provider: MigrationProvider,
	detection: ProviderDetection,
): MigrationCategory[] {
	switch (provider) {
		case "claude-code":
			return buildClaudeCodeCategories(detection)
		case "cursor":
			return buildCursorCategories(detection)
		case "devo":
			return buildDevoCategories(detection)
		case "opencode":
			return buildOpenCodeCategories(detection)
	}
}

function buildClaudeCodeCategories(detection: ProviderDetection): MigrationCategory[] {
	const historyParts: string[] = []
	if (detection.projectCount > 0) {
		historyParts.push(`${detection.projectCount} project${detection.projectCount === 1 ? "" : "s"}`)
	}
	if (detection.totalSessions > 0) {
		historyParts.push(
			`${detection.totalSessions} session${detection.totalSessions === 1 ? "" : "s"}`,
		)
	}

	return [
		{
			id: "config",
			label: "Global settings & model preferences",
			description: "Model IDs, provider config, auto-update settings",
			icon: CogIcon,
			count: detection.hasGlobalSettings ? 1 : 0,
			enabled: true,
		},
		{
			id: "mcp",
			label: "MCP server configurations",
			description: "Local and remote MCP server definitions",
			icon: ServerIcon,
			count: detection.mcpServerCount,
			enabled: true,
		},
		{
			id: "history",
			label: "Projects & sessions",
			description: historyParts.length > 0 ? historyParts.join(", ") : "No sessions found",
			icon: FolderOpenIcon,
			count: detection.totalSessions,
			enabled: detection.totalSessions > 0,
		},
		{
			id: "agents",
			label: "Custom agents",
			description: "Agent definitions with tools and model preferences",
			icon: BotIcon,
			count: detection.agentCount,
			enabled: true,
		},
		{
			id: "commands",
			label: "Custom commands",
			description: "Command templates with parameters",
			icon: TerminalIcon,
			count: detection.commandCount,
			enabled: true,
		},
		{
			id: "rules",
			label: "Project rules (CLAUDE.md)",
			description: "Copied as AGENTS.md for Devo",
			icon: ScrollTextIcon,
			count: detection.ruleCount,
			enabled: true,
		},
		{
			id: "permissions",
			label: "Permission settings",
			description: "Tool allow/deny/ask rules",
			icon: ShieldIcon,
			count: detection.hasGlobalSettings ? 1 : 0,
			enabled: true,
		},
		{
			id: "hooks",
			label: "Hooks",
			description: "Converted to TypeScript plugin stubs (manual finishing needed)",
			icon: PlugIcon,
			count: detection.hasHooks ? 1 : 0,
			enabled: true,
		},
		{
			id: "skills",
			label: "Skills",
			description: "Verified for compatibility",
			icon: FileTextIcon,
			count: detection.skillCount,
			enabled: true,
		},
	]
}

function buildCursorCategories(detection: ProviderDetection): MigrationCategory[] {
	const historyParts: string[] = []
	if (detection.totalSessions > 0) {
		historyParts.push(
			`${detection.totalSessions} session${detection.totalSessions === 1 ? "" : "s"}`,
		)
	}
	if (detection.totalMessages > 0) {
		historyParts.push(
			`${detection.totalMessages} message${detection.totalMessages === 1 ? "" : "s"}`,
		)
	}

	return [
		{
			id: "config",
			label: "Global settings & permissions",
			description: "CLI permissions and configuration",
			icon: CogIcon,
			count: detection.hasGlobalSettings ? 1 : 0,
			enabled: true,
		},
		{
			id: "mcp",
			label: "MCP server configurations",
			description: "Local and remote MCP server definitions",
			icon: ServerIcon,
			count: detection.mcpServerCount,
			enabled: true,
		},
		{
			id: "history",
			label: "Chat history",
			description: historyParts.length > 0 ? historyParts.join(", ") : "No chat sessions found",
			icon: FolderOpenIcon,
			count: detection.totalSessions,
			enabled: detection.totalSessions > 0,
		},
		{
			id: "agents",
			label: "Custom agents",
			description: "Agent definitions from .cursor/agents/",
			icon: BotIcon,
			count: detection.agentCount,
			enabled: true,
		},
		{
			id: "commands",
			label: "Custom commands",
			description: "Command files from .cursor/commands/",
			icon: TerminalIcon,
			count: detection.commandCount,
			enabled: true,
		},
		{
			id: "rules",
			label: "Rules (.mdc files)",
			description: "Cursor rules converted to AGENTS.md format",
			icon: ScrollTextIcon,
			count: detection.ruleCount,
			enabled: true,
		},
		{
			id: "permissions",
			label: "Permission settings",
			description: "CLI agent permissions from cli-config.json",
			icon: ShieldIcon,
			count: detection.hasPermissions ? 1 : 0,
			enabled: true,
		},
		{
			id: "skills",
			label: "Skills",
			description: "Verified for compatibility",
			icon: FileTextIcon,
			count: detection.skillCount,
			enabled: true,
		},
	]
}

function buildDevoCategories(detection: ProviderDetection): MigrationCategory[] {
	return [
		{
			id: "config",
			label: "Global configuration",
			description: "devo.json settings and model preferences",
			icon: CogIcon,
			count: detection.hasGlobalSettings ? 1 : 0,
			enabled: true,
		},
		{
			id: "mcp",
			label: "MCP server configurations",
			description: "Local and remote MCP server definitions",
			icon: ServerIcon,
			count: detection.mcpServerCount,
			enabled: true,
		},
		{
			id: "agents",
			label: "Custom agents",
			description: "Agent definitions from .devo/agents/",
			icon: BotIcon,
			count: detection.agentCount,
			enabled: true,
		},
		{
			id: "commands",
			label: "Custom commands",
			description: "Command files from .devo/commands/",
			icon: TerminalIcon,
			count: detection.commandCount,
			enabled: true,
		},
		{
			id: "rules",
			label: "Rules (AGENTS.md)",
			description: "Agent instructions and project rules",
			icon: ScrollTextIcon,
			count: detection.ruleCount,
			enabled: true,
		},
		{
			id: "skills",
			label: "Skills",
			description: "Verified for compatibility",
			icon: FileTextIcon,
			count: detection.skillCount,
			enabled: true,
		},
	]
}

function buildOpenCodeCategories(detection: ProviderDetection): MigrationCategory[] {
	return [
		{
			id: "config",
			label: "Providers & models",
			description: "Provider base URLs, API keys, and model bindings",
			icon: CogIcon,
			count: detection.hasGlobalSettings ? 1 : 0,
			enabled: true,
		},
	]
}
