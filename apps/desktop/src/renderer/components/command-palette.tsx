import {
	CommandDialog,
	CommandEmpty,
	CommandGroup,
	CommandInput,
	CommandItem,
	CommandList,
	CommandSeparator,
	CommandShortcut,
} from "@devo/ui/components/command"
import { useNavigate, useParams } from "@tanstack/react-router"
import { useAtom, useAtomValue, useSetAtom } from "jotai"
import {
	BotIcon,
	CheckIcon,
	CloudIcon,
	ContainerIcon,
	EyeIcon,
	EyeOffIcon,
	FilmIcon,
	GitBranchIcon,
	GitForkIcon,
	MonitorIcon,
	MoonIcon,
	PaletteIcon,
	PlusIcon,
	Redo2Icon,
	RefreshCwIcon,
	ScanEyeIcon,
	SparklesIcon,
	SunIcon,
	SunMoonIcon,
	Undo2Icon,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useState } from "react"
import { sessionMetricsFamily } from "../atoms/derived/session-metrics"
import { automationsEnabledAtom, toggleAutomationsAtom } from "../atoms/feature-flags"
import { isMockModeAtom, toggleMockModeAtom } from "../atoms/mock-mode"
import { lastProjectDirectoryAtom, opaqueWindowsAtom } from "../atoms/preferences"
import { isReactScanAtom, toggleReactScanAtom } from "../atoms/react-scan"
import { useSessionRevert } from "../hooks/use-commands"
import { useProjectList } from "../hooks/use-agents"
import {
	useAvailableThemes,
	useColorScheme,
	useCurrentTheme,
	useSetColorScheme,
	useSetTheme,
} from "../hooks/use-theme"
import { createLogger } from "../lib/logger"
import { formatShortcut } from "../lib/shortcut-display"
import { navigateToNewChat } from "../lib/project-selection"
import type { ColorScheme } from "../lib/themes"
import type { Agent } from "../lib/types"
import { reloadConfig } from "../services/connection-manager"

interface CommandPaletteProps {
	open: boolean
	onOpenChange: (open: boolean) => void
	agents: Agent[]
	/** Fork the currently active session (full fork) */
	onForkSession?: () => Promise<void>
}

const log = createLogger("command-palette")

export function CommandPalette({ open, onOpenChange, agents, onForkSession }: CommandPaletteProps) {
	const navigate = useNavigate()
	const params = useParams({ strict: false })
	const sessionId = (params as Record<string, string | undefined>).sessionId ?? null
	const projectSlug = (params as Record<string, string | undefined>).projectSlug
	const projects = useProjectList()
	const lastProjectDirectory = useAtomValue(lastProjectDirectoryAtom)

	// Resolve the active session's directory for undo/redo
	const activeAgent = useMemo(
		() => (sessionId ? (agents.find((a) => a.id === sessionId) ?? null) : null),
		[agents, sessionId],
	)
	const directory = activeAgent?.directory ?? null

	const { canUndo, canRedo, undo, redo } = useSessionRevert(
		directory,
		activeAgent?.sessionId ?? null,
	)

	// Theme & color scheme state
	const currentTheme = useCurrentTheme()
	const colorScheme = useColorScheme()
	const availableThemes = useAvailableThemes()
	const setTheme = useSetTheme()
	const setColorScheme = useSetColorScheme()
	const [opaqueWindows, setOpaqueWindows] = useAtom(opaqueWindowsAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const toggleMockMode = useSetAtom(toggleMockModeAtom)
	const isReactScan = useAtomValue(isReactScanAtom)
	const toggleReactScan = useSetAtom(toggleReactScanAtom)
	const automationsEnabled = useAtomValue(automationsEnabledAtom)
	const toggleAutomations = useSetAtom(toggleAutomationsAtom)
	const [reloading, setReloading] = useState(false)
	const newSessionShortcut = formatShortcut(["mod", "N"])
	const undoShortcut = formatShortcut(["mod", "Z"])
	const redoShortcut = formatShortcut(["shift", "mod", "Z"])

	const isElectron = typeof window !== "undefined" && "devo" in window

	const handleToggleTransparency = useCallback(async () => {
		const newValue = !opaqueWindows
		setOpaqueWindows(newValue)

		// Persist to main process so the next window creation uses the correct chrome tier
		if (isElectron) {
			await window.devo.setOpaqueWindows(newValue)
			// BrowserWindow.transparent is a creation-time option — prompt for restart
			const shouldRestart = window.confirm(
				"Transparency changes take effect after restarting the app.\n\nRestart now?",
			)
			if (shouldRestart) {
				window.devo.relaunch()
			}
		}
	}, [opaqueWindows, setOpaqueWindows, isElectron])

	const handleReloadConfig = useCallback(async () => {
		setReloading(true)
		onOpenChange(false)
		try {
			await reloadConfig()
			log.info("Config reloaded successfully")
		} catch (err) {
			log.error("Failed to reload config", {}, err)
		} finally {
			setReloading(false)
		}
	}, [onOpenChange])

	useEffect(() => {
		function handleKeyDown(e: KeyboardEvent) {
			if (e.key === "k" && (e.metaKey || e.ctrlKey)) {
				e.preventDefault()
				onOpenChange(!open)
			}
		}
		document.addEventListener("keydown", handleKeyDown)
		return () => document.removeEventListener("keydown", handleKeyDown)
	}, [open, onOpenChange])

	const activeSessions = useMemo(
		() => (open ? agents.filter((a) => a.status === "running" || a.status === "waiting") : []),
		[agents, open],
	)

	// Whether session-level commands should be shown
	const hasSession = !!activeAgent

	return (
		<CommandDialog open={open} onOpenChange={onOpenChange}>
			<CommandInput placeholder="Type a command or search..." />
			<CommandList>
				<CommandEmpty>No results found.</CommandEmpty>

				<CommandGroup heading="Actions">
					<CommandItem
						onSelect={() => {
							navigateToNewChat(navigate, projects, projectSlug, lastProjectDirectory)
							onOpenChange(false)
						}}
					>
						<PlusIcon />
						<span>New Session</span>
						<CommandShortcut>{newSessionShortcut}</CommandShortcut>
					</CommandItem>
					{hasSession && canUndo && (
						<CommandItem
							onSelect={() => {
								undo()
								onOpenChange(false)
							}}
						>
							<Undo2Icon />
							<span>Undo Last Turn</span>
							<CommandShortcut>{undoShortcut}</CommandShortcut>
						</CommandItem>
					)}
					{hasSession && canRedo && (
						<CommandItem
							onSelect={() => {
								redo()
								onOpenChange(false)
							}}
						>
							<Redo2Icon />
							<span>Redo</span>
							<CommandShortcut>{redoShortcut}</CommandShortcut>
						</CommandItem>
					)}
				{hasSession && (
					<CommandItem
						onSelect={() => {
							// Compact is handled via slash command — just close and navigate
							onOpenChange(false)
						}}
						disabled
					>
						<SparklesIcon />
						<span>Compact Conversation</span>
					</CommandItem>
				)}
				{hasSession && onForkSession && (
					<CommandItem
						onSelect={async () => {
							onOpenChange(false)
							await onForkSession()
						}}
					>
						<GitForkIcon />
						<span>Fork Session</span>
					</CommandItem>
				)}
					<CommandItem onSelect={handleReloadConfig} disabled={reloading}>
						<RefreshCwIcon />
						<span>{reloading ? "Reloading..." : "Reload Config"}</span>
					</CommandItem>
				</CommandGroup>

				<CommandSeparator />
				<CommandGroup heading="Appearance">
					{availableThemes.map((t) => (
						<CommandItem
							key={t.id}
							onSelect={() => {
								setTheme(t.id)
								onOpenChange(false)
							}}
						>
							<PaletteIcon />
							<span>Theme: {t.name}</span>
							{t.description && (
								<span className="text-xs text-muted-foreground">{t.description}</span>
							)}
							{currentTheme.id === t.id && <CheckIcon className="ml-auto h-4 w-4" />}
						</CommandItem>
					))}
				</CommandGroup>

				<CommandSeparator />
				<CommandGroup heading="Window">
					<CommandItem
						onSelect={() => {
							onOpenChange(false)
							// Small delay so the palette closes before the confirm dialog appears
							setTimeout(handleToggleTransparency, 100)
						}}
					>
						{opaqueWindows ? <EyeIcon /> : <EyeOffIcon />}
						<span>{opaqueWindows ? "Enable Transparency" : "Disable Transparency"}</span>
						{!opaqueWindows && <CheckIcon className="ml-auto h-4 w-4" />}
					</CommandItem>
				</CommandGroup>

				<CommandSeparator />
				<CommandGroup heading="Color Scheme">
					{(
						[
							{ scheme: "dark" as ColorScheme, label: "Dark", icon: MoonIcon },
							{ scheme: "light" as ColorScheme, label: "Light", icon: SunIcon },
							{ scheme: "system" as ColorScheme, label: "System", icon: SunMoonIcon },
						] as const
					).map(({ scheme, label, icon: Icon }) => (
						<CommandItem
							key={scheme}
							onSelect={() => {
								setColorScheme(scheme)
								onOpenChange(false)
							}}
						>
							<Icon />
							<span>{label}</span>
							{colorScheme === scheme && <CheckIcon className="ml-auto h-4 w-4" />}
						</CommandItem>
					))}
				</CommandGroup>

				<CommandSeparator />
				<CommandGroup heading="Features">
					<CommandItem
						keywords={["automation", "automations", "schedule", "cron", "recurring"]}
						onSelect={() => {
							toggleAutomations()
							onOpenChange(false)
						}}
					>
						<BotIcon />
						<span>{automationsEnabled ? "Disable Automations" : "Enable Automations"}</span>
						{automationsEnabled && <CheckIcon className="ml-auto h-4 w-4" />}
					</CommandItem>
				</CommandGroup>

				<CommandSeparator />
				<CommandGroup heading="Developer">
					<CommandItem
						keywords={["demo", "mock", "screenshot", "marketing"]}
						onSelect={() => {
							toggleMockMode()
							onOpenChange(false)
						}}
					>
						<FilmIcon />
						<span>{isMockMode ? "Disable Demo Mode" : "Enable Demo Mode"}</span>
						{isMockMode && <CheckIcon className="ml-auto h-4 w-4" />}
					</CommandItem>
					{import.meta.env.DEV && (
						<CommandItem
							keywords={["react", "scan", "render", "rerender", "performance", "debug"]}
							onSelect={() => {
								toggleReactScan()
								onOpenChange(false)
							}}
						>
							<ScanEyeIcon />
							<span>{isReactScan ? "Disable React Scan" : "Enable React Scan"}</span>
							{isReactScan && <CheckIcon className="ml-auto h-4 w-4" />}
						</CommandItem>
					)}
				</CommandGroup>

				{activeSessions.length > 0 && (
					<>
						<CommandSeparator />
						<CommandGroup heading="Active Sessions">
							{activeSessions.map((agent) => (
								<CommandItem
									key={agent.id}
									onSelect={() => {
										navigate({
											to: "/project/$projectSlug/session/$sessionId",
											params: { projectSlug: agent.projectSlug, sessionId: agent.id },
										})
										onOpenChange(false)
									}}
								>
									{agent.environment === "cloud" ? (
										<CloudIcon />
									) : agent.environment === "vm" ? (
										<ContainerIcon />
									) : (
										<MonitorIcon />
									)}
									<span>{agent.name}</span>
									<span className="text-xs text-muted-foreground">{agent.project}</span>
								</CommandItem>
							))}
						</CommandGroup>
					</>
				)}

				{agents.length > 0 && (
					<>
						<CommandSeparator />
						<CommandGroup heading="All Sessions">
							{agents.map((agent) => (
								<CommandItem
									key={agent.id}
									onSelect={() => {
										navigate({
											to: "/project/$projectSlug/session/$sessionId",
											params: { projectSlug: agent.projectSlug, sessionId: agent.id },
										})
										onOpenChange(false)
									}}
								>
									<GitBranchIcon />
									<span>{agent.name}</span>
									<SessionMetricsLabel sessionId={agent.id} project={agent.project} />
								</CommandItem>
							))}
						</CommandGroup>
					</>
				)}
			</CommandList>
		</CommandDialog>
	)
}

/**
 * Small leaf component that reads session metrics for display in the command palette.
 * Only subscribes to the individual session's metrics atom, so metrics are only
 * computed for sessions visible in the command palette (and only when it's open).
 */
function SessionMetricsLabel({ sessionId, project }: { sessionId: string; project: string }) {
	const metrics = useAtomValue(sessionMetricsFamily(sessionId))
	return (
		<span className="text-xs text-muted-foreground">
			{project} &middot; {metrics.workTime}
			{metrics.costRaw > 0 && ` · ${metrics.cost}`}
		</span>
	)
}
