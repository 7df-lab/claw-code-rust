/**
 * Root layout: shared providers, global hooks, keyboard navigation,
 * command palette, and onboarding.
 * Does NOT render any sidebar chrome -- that lives in SidebarLayout.
 */
import { TooltipProvider } from "@devo/ui/components/tooltip"
import { Outlet, useNavigate, useParams } from "@tanstack/react-router"
import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useEffect, useMemo } from "react"
import { Toaster } from "sonner"
import { discoveryPhaseAtom } from "../atoms/discovery"
import { onboardingStateAtom } from "../atoms/onboarding"
import { terminalPanelOpenAtom } from "../atoms/terminal"
import { lastProjectDirectoryAtom } from "../atoms/preferences"
import { customizeOpenAtom } from "../atoms/ui"
import { useAgents, useCommandPaletteOpen, useProjectList, useSetCommandPaletteOpen } from "../hooks/use-agents"
import { useChromeTier } from "../hooks/use-chrome-tier"
import { useDesktopSettingsSync } from "../hooks/use-desktop-settings-sync"
import { useDiscovery } from "../hooks/use-discovery"
import { useMockMode } from "../hooks/use-mock-mode"
import { useNotifications } from "../hooks/use-notifications"
import { useAgentActions, useServerConnection } from "../hooks/use-server"
import { useServerSettingsSync } from "../hooks/use-servers"
import { useSystemAccentColor } from "../hooks/use-system-accent-color"
import { useThemeEffect } from "../hooks/use-theme"
import { useWaitingIndicator } from "../hooks/use-waiting-indicator"
import { navigateToNewChat } from "../lib/project-selection"
import { isTerminalToggleShortcut } from "../lib/terminal-shortcut"
import { AppBarProvider } from "./app-bar-context"
import { CommandPalette } from "./command-palette"
import { OnboardingOverlay } from "./onboarding/onboarding-overlay"
import { SidebarSlotProvider } from "./sidebar-slot-context"
import { StartupOverlay } from "./startup-overlay"

export function RootLayout() {
	const isMockMode = useMockMode()
	const onboardingState = useAtomValue(onboardingStateAtom)
	const setOnboardingState = useSetAtom(onboardingStateAtom)

	// Only run discovery/connection after onboarding is complete (or in browser mode / mock mode)
	const isElectronEnv = typeof window !== "undefined" && "devo" in window
	const showOnboarding = isElectronEnv && !onboardingState.completed && !isMockMode

	// Track discovery phase to coordinate startup overlay / content crossfade
	const phase = useAtomValue(discoveryPhaseAtom)

	useServerSettingsSync()
	useDesktopSettingsSync()
	useDiscovery()
	useServerConnection()
	useWaitingIndicator()
	useThemeEffect()
	useChromeTier()
	useSystemAccentColor()

	const agents = useAgents()
	const projects = useProjectList()
	const lastProjectDirectory = useAtomValue(lastProjectDirectoryAtom)
	const setCustomizeOpen = useSetAtom(customizeOpenAtom)
	const { forkSession } = useAgentActions()
	const commandPaletteOpen = useCommandPaletteOpen()
	const setCommandPaletteOpen = useSetCommandPaletteOpen()
	const setTerminalPanelOpen = useSetAtom(terminalPanelOpenAtom)
	const navigate = useNavigate()
	const params = useParams({ strict: false })
	const sessionId = (params as Record<string, string | undefined>).sessionId
	const projectSlug = (params as Record<string, string | undefined>).projectSlug

	// Native OS notifications: badge sync, click-to-navigate, auto-dismiss
	useNotifications(navigate, sessionId, projectSlug)

	// ========== Command palette: fork session ==========

	const activeAgent = useMemo(
		() => (sessionId ? (agents.find((a) => a.id === sessionId) ?? null) : null),
		[agents, sessionId],
	)

	const handleForkSession = useCallback(async () => {
		if (!activeAgent) return
		const forked = await forkSession(activeAgent.directory, activeAgent.id)
		navigate({
			to: "/project/$projectSlug/session/$sessionId",
			params: { projectSlug: activeAgent.projectSlug, sessionId: forked.id },
		})
	}, [activeAgent, forkSession, navigate])

	// Sub-agents are filtered at the API level (roots: true), so all agents here are root agents
	const visibleAgents = agents

	// ========== Keyboard navigation ==========

	const handleKeyDown = useCallback(
		(e: KeyboardEvent) => {
			if (isTerminalToggleShortcut(e)) {
				e.preventDefault()
				setTerminalPanelOpen((open) => !open)
				return
			}

			const target = e.target as HTMLElement
			if (target.tagName === "INPUT" || target.tagName === "TEXTAREA" || target.isContentEditable) {
				return
			}

			if (e.key === "Escape") {
				e.preventDefault()
				navigate({ to: "/" })
				return
			}

			if ((e.key === "j" || e.key === "k") && !e.metaKey && !e.ctrlKey && !e.altKey) {
				e.preventDefault()
				const currentIndex = visibleAgents.findIndex((a) => a.id === sessionId)
				let nextIndex: number
				if (e.key === "j") {
					nextIndex = currentIndex < visibleAgents.length - 1 ? currentIndex + 1 : 0
				} else {
					nextIndex = currentIndex > 0 ? currentIndex - 1 : visibleAgents.length - 1
				}
				const agent = visibleAgents[nextIndex]
				if (agent) {
					navigate({
						to: "/project/$projectSlug/session/$sessionId",
						params: {
							projectSlug: agent.projectSlug,
							sessionId: agent.id,
						},
					})
				}
				return
			}

			if ((e.metaKey || e.ctrlKey) && e.key === "n") {
				e.preventDefault()
				setCustomizeOpen(false)
				navigateToNewChat(navigate, projects, projectSlug, lastProjectDirectory)
				return
			}

			if ((e.metaKey || e.ctrlKey) && e.key === "k") {
				e.preventDefault()
				setCommandPaletteOpen(true)
				return
			}
		},
		[lastProjectDirectory, navigate, projectSlug, projects, sessionId, setCommandPaletteOpen, setCustomizeOpen, setTerminalPanelOpen, visibleAgents],
	)

	useEffect(() => {
		document.addEventListener("keydown", handleKeyDown, { capture: true })
		return () => document.removeEventListener("keydown", handleKeyDown, { capture: true })
	}, [handleKeyDown])

	useEffect(() => {
		if (typeof window === "undefined" || !("devo" in window)) return
		if (typeof window.devo.onTerminalToggle !== "function") return
		return window.devo.onTerminalToggle(() => {
			setTerminalPanelOpen((open) => !open)
		})
	}, [setTerminalPanelOpen])

	// ========== Onboarding completion ==========

	const handleOnboardingComplete = useCallback(
		(state: {
			skippedSteps: string[]
			migrationPerformed: boolean
			migratedFrom: string[]
			devoVersion: string | null
			providersConnected: number
		}) => {
			setOnboardingState({
				completed: true,
				completedAt: new Date().toISOString(),
				skippedSteps: state.skippedSteps,
				migrationPerformed: state.migrationPerformed,
				migratedFrom: state.migratedFrom,
				devoVersion: state.devoVersion,
				providersConnected: state.providersConnected,
			})
		},
		[setOnboardingState],
	)

	// ========== Splash cleanup during onboarding ==========
	// The HTML-level #splash (index.html) is normally removed by StartupOverlay's
	// mount effect. When onboarding is shown we return early and StartupOverlay
	// never mounts, so clean up the HTML splash here instead.
	useEffect(() => {
		if (!showOnboarding) return
		const splash = document.getElementById("splash")
		if (splash) {
			splash.classList.add("hiding")
			setTimeout(() => splash.remove(), 300)
		}
	}, [showOnboarding])

	// ========== Layout ==========

	if (showOnboarding) {
		return <OnboardingOverlay onComplete={handleOnboardingComplete} />
	}

	// Hide app content while the startup overlay is covering the screen.
	// The overlay fades out at "ready"; showing content at "ready" creates a
	// smooth crossfade. Content is still rendered (just invisible) so React
	// can paint it before the overlay lifts.
	const contentReady = phase === "ready" || phase === "loading-sessions" || phase === "error"

	return (
		<TooltipProvider>
			<AppBarProvider>
				<SidebarSlotProvider>
					<div
						className={`transition-opacity duration-300 ${contentReady ? "opacity-100" : "opacity-0"}`}
					>
						<Outlet />
						<CommandPalette
							open={commandPaletteOpen}
							onOpenChange={setCommandPaletteOpen}
							agents={agents}
							onForkSession={activeAgent ? handleForkSession : undefined}
						/>
						<Toaster position="bottom-right" />
					</div>
					<StartupOverlay />
				</SidebarSlotProvider>
			</AppBarProvider>
		</TooltipProvider>
	)
}
