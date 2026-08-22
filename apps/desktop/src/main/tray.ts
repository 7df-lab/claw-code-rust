/**
 * Dynamic system tray for Devo.
 *
 * Shows live agent statuses grouped by project, pending action counts,
 * and quick-access actions. Rebuilds the context menu whenever session
 * state changes via the notification-watcher's Native event stream.
 *
 * macOS features:
 * - Full-color tray icon that preserves the Devo black rounded-square mark
 * - Tray title badge showing pending permission/question count
 * - Status indicators via Unicode symbols (●/◐/○)
 */
import fs from "node:fs"
import path from "node:path"
import { fileURLToPath } from "node:url"
import { createDevoClient, type Project, type Session } from "@devo-ai/sdk/v2/client"
import { app, type BrowserWindow, Menu, nativeImage, Tray } from "electron"
import { createLogger } from "./logger"
import {
	getPendingCount,
	getSessionStates,
	onStateChanged,
	type SessionState,
} from "./notification-watcher"
import { getNativeTransport, getServerUrl, onServerReady } from "./devo-manager"
import { buildDevoTrayMenuTemplate, type DiscoveryCache } from "./tray-menu"

const log = createLogger("tray")

// ESM equivalent for __dirname
const __filename = fileURLToPath(import.meta.url)
const __dirname = path.dirname(__filename)

// ============================================================
// Constants
// ============================================================

const IS_MAC = process.platform === "darwin"
const IS_LINUX = process.platform === "linux"

/** How often to refresh discovery data (offline sessions). */
const DISCOVERY_REFRESH_MS = 60_000
const DESKTOP_TRAY_ICON_SIZE = 18
const LINUX_TRAY_ICON_SIZE = 22

// ============================================================
// State
// ============================================================

let tray: Tray | null = null
let getWindow: (() => BrowserWindow | undefined) | null = null
let unsubscribeWatcher: (() => void) | null = null
let unsubscribeServerReady: (() => void) | null = null
let discoveryCache: DiscoveryCache | null = null
let discoveryTimer: ReturnType<typeof setInterval> | null = null
let discoveryRefreshInFlight: Promise<void> | null = null

interface TrayInteractionTarget {
	on(event: string, listener: (...args: unknown[]) => void): unknown
}

interface TrayInteractionHandlers {
	showWindow: () => void
}

// ============================================================
// Public API
// ============================================================

export function createTray(windowGetter: () => BrowserWindow | undefined): void {
	if (tray) return

	getWindow = windowGetter

	const resourcesPath = app.isPackaged
		? process.resourcesPath
		: path.join(__dirname, "../../resources")

	const icon = createTrayIcon(resourcesPath)

	if (icon.isEmpty()) {
		log.error("Tray icon is empty — file may be missing or corrupt")
	}

	tray = new Tray(icon)
	tray.setToolTip("Devo")
	installTrayIconInteractions(tray, { showWindow })

	// Subscribe to notification-watcher state changes for live updates
	unsubscribeWatcher = onStateChanged(() => {
		rebuildMenu()
	})
	unsubscribeServerReady = onServerReady(() => {
		void refreshDiscovery()
	})

	// Load discovery data for offline sessions, then refresh periodically
	refreshDiscovery()
	discoveryTimer = setInterval(refreshDiscovery, DISCOVERY_REFRESH_MS)

	// Build initial menu
	rebuildMenu()

	log.info(`Tray created (platform: ${IS_MAC ? "macOS" : IS_LINUX ? "Linux" : "Windows"})`)
}

export function createTrayIcon(
	resourcesPath: string,
	platform: NodeJS.Platform = process.platform,
): Electron.NativeImage {
	if (platform === "darwin" || platform === "win32") {
		const trayIconPath = path.join(resourcesPath, "iconTray.png")
		if (!fs.existsSync(trayIconPath)) {
			log.error(`Tray icon not found at ${trayIconPath} — tray will be invisible`)
		}
		return nativeImage
			.createFromPath(trayIconPath)
			.resize({ width: DESKTOP_TRAY_ICON_SIZE, height: DESKTOP_TRAY_ICON_SIZE })
	}

	if (platform === "linux") {
		// Linux: use 22x22 icon (standard tray size), fallback to icon.png if not available.
		// Explicitly resize to 22x22 to ensure the GTK pixbuf is in a format the
		// StatusNotifierItem (SNI) protocol on Wayland can handle — avoids
		// GDK_IS_PIXBUF assertion failures from malformed pixbuf handoff.
		const trayIconPath = path.join(resourcesPath, "iconTray.png")
		if (!fs.existsSync(trayIconPath)) {
			log.warn(`Tray icon not found at ${trayIconPath} — falling back to icon.png`)
		}
		const rawIcon = fs.existsSync(trayIconPath)
			? nativeImage.createFromPath(trayIconPath)
			: nativeImage.createFromPath(path.join(resourcesPath, "icon.png"))
		return rawIcon.resize({ width: LINUX_TRAY_ICON_SIZE, height: LINUX_TRAY_ICON_SIZE })
	}

	const iconPath = path.join(resourcesPath, "icon.png")
	if (!fs.existsSync(iconPath)) {
		log.error(`Tray icon not found at ${iconPath} — tray will be invisible`)
	}
	return nativeImage.createFromPath(iconPath)
}

export function installTrayIconInteractions(
	tray: TrayInteractionTarget,
	handlers: TrayInteractionHandlers,
	platform: NodeJS.Platform = process.platform,
): void {
	if (platform !== "win32") return

	// Windows users expect a left-click on the tray icon to restore the app.
	tray.on("click", handlers.showWindow)
}

export function destroyTray(): void {
	if (unsubscribeWatcher) {
		unsubscribeWatcher()
		unsubscribeWatcher = null
	}
	if (unsubscribeServerReady) {
		unsubscribeServerReady()
		unsubscribeServerReady = null
	}
	if (discoveryTimer) {
		clearInterval(discoveryTimer)
		discoveryTimer = null
	}
	if (tray) {
		tray.destroy()
		tray = null
	}
	getWindow = null
	discoveryCache = null
}

// ============================================================
// Menu Building
// ============================================================

function rebuildMenu(): void {
	if (!tray) return

	const liveSessions = getSessionStates()
	const pendingCount = getPendingCount()

	const template = buildDevoTrayMenuTemplate({
		liveSessions,
		discovery: discoveryCache,
		pendingCount,
		onNavigateToSession: navigateToSession,
		onNewChat: navigateToNewChat,
		onOpenDevo: showWindow,
		onQuitDevo: () => app.quit(),
	})

	const contextMenu = Menu.buildFromTemplate(template)
	tray.setContextMenu(contextMenu)

	// macOS: show pending count next to tray icon
	updateTrayTitle(pendingCount, liveSessions)
}

// ============================================================
// Tray Title / Icon State (macOS)
// ============================================================

function updateTrayTitle(
	pendingCount: number,
	liveSessions: ReadonlyMap<string, SessionState>,
): void {
	if (!tray) return

	if (IS_MAC) {
		// Show counts next to the tray icon
		const busyCount = Array.from(liveSessions.values()).filter(
			(s) => !s.parentID && (s.status === "busy" || s.status === "retry"),
		).length

		let title = ""
		if (pendingCount > 0) {
			title = `${pendingCount}!`
		} else if (busyCount > 0) {
			title = `${busyCount}`
		}

		tray.setTitle(title, { fontType: "monospacedDigit" })
	}

	// Update tooltip with summary
	const totalSessions = Array.from(liveSessions.values()).filter((s) => !s.parentID).length
	const busyCount = Array.from(liveSessions.values()).filter(
		(s) => !s.parentID && (s.status === "busy" || s.status === "retry"),
	).length

	let tooltip = "Devo"
	if (totalSessions > 0) {
		tooltip += ` - ${totalSessions} agent${totalSessions !== 1 ? "s" : ""}`
		if (busyCount > 0) {
			tooltip += ` (${busyCount} running)`
		}
	}
	if (pendingCount > 0) {
		tooltip += ` - ${pendingCount} pending`
	}
	tray.setToolTip(tooltip)
}

// ============================================================
// Discovery Data — fetched from Devo API via SDK
// ============================================================

async function refreshDiscovery(): Promise<void> {
	const serverUrl = getServerUrl()
	if (!serverUrl) return
	if (discoveryRefreshInFlight) return discoveryRefreshInFlight

	discoveryRefreshInFlight = refreshDiscoveryForServer().finally(() => {
		discoveryRefreshInFlight = null
	})
	return discoveryRefreshInFlight
}

async function refreshDiscoveryForServer(): Promise<void> {
	try {
		const client = createDevoClient({ transport: getNativeTransport() })
		const [projectsResult, sessionsResult] = await Promise.all([
			client.project.list(),
			client.session.list({ roots: true }),
		])

		discoveryCache = {
			projects: (projectsResult.data ?? []) as Project[],
			sessions: (sessionsResult.data ?? []) as Session[],
		}

		// Rebuild menu with fresh discovery data
		rebuildMenu()
	} catch (err) {
		log.warn("Failed to refresh discovery data for tray", err)
	}
}

// ============================================================
// Navigation & Window Helpers
// ============================================================

function showWindow(): void {
	const win = getWindow?.()
	if (win) {
		if (win.isMinimized()) win.restore()
		win.show()
		win.focus()
	}
}

function navigateToSession(sessionId: string): void {
	const win = getWindow?.()
	if (win) {
		if (win.isMinimized()) win.restore()
		win.show()
		win.focus()
		win.webContents.send("notification:navigate", { sessionId })
	}
}

function navigateToNewChat(): void {
	const win = getWindow?.()
	if (win) {
		if (win.isMinimized()) win.restore()
		win.show()
		win.focus()
		win.webContents.send("tray:new-chat")
	}
}
