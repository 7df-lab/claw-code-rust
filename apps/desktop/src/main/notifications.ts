import { app, BrowserWindow, Notification } from "electron"
import { createLogger } from "./logger"
import { isAppInForeground } from "./notification-policy"
import { getNotificationSettings } from "./settings-store"

const log = createLogger("notifications")

// ============================================================
// Types
// ============================================================

export interface NotificationRequest {
	type: "permission" | "question" | "completed" | "error"
	sessionId: string
	title: string
	body: string
	/** Directory associated with the session (used for permission reply). */
	directory?: string
	meta?: {
		permissionId?: string
		requestId?: string
		sessionTitle?: string
	}
}

type PermissionResponder = (params: {
	sessionId: string
	permissionId: string
	response: "once" | "always" | "reject"
	directory?: string
}) => Promise<void>

/** Permission reply bridge, set by the notification watcher when it connects. */
let permissionResponder: PermissionResponder | null = null

/** Set the permission responder for notification action replies. */
export function setPermissionResponder(responder: PermissionResponder | null): void {
	permissionResponder = responder
}

// ============================================================
// State
// ============================================================

const BATCH_WINDOW_MS = 3_000
const COOLDOWN_MS = 30_000

/** Tracks when we last showed a notification per session+type to prevent spam. */
const lastShown = new Map<string, number>()

/** Batches rapid-fire permission notifications per session. */
const batchQueue = new Map<string, NotificationRequest[]>()
const batchTimers = new Map<string, ReturnType<typeof setTimeout>>()

/** Active notifications keyed by session ID for dismiss-on-navigate. */
const activeNotifications = new Map<string, Notification>()

// ============================================================
// Public API
// ============================================================

/**
 * Show a native OS notification for an agent event.
 *
 * Applies suppression rules:
 * 1. Skip if Notification API is not supported
 * 2. Skip if the app window is focused (user is already looking)
 * 3. Skip if we recently showed a notification for the same session+type
 * 4. Batch rapid-fire permission requests into a single notification
 */
export function showNotification(request: NotificationRequest): void {
	if (!Notification.isSupported()) {
		log.debug("Notification API not supported on this host")
		return
	}

	// Check if this notification type is enabled in settings
	const prefs = getNotificationSettings()
	if (request.type === "permission" && !prefs.permissions) return
	if (request.type === "question" && !prefs.questions) return
	if (request.type === "error" && !prefs.errors) return
	if (request.type === "completed") {
		if (prefs.completionMode === "off") return
		// "always" mode: skip the focused check below
		// "unfocused" mode: normal behavior (suppress when focused)
	}

	// Suppression: a visible focused window means the user is already looking.
	if (isAppInForeground(BrowserWindow.getAllWindows()) && !(request.type === "completed" && prefs.completionMode === "always")) {
		log.debug("Suppressed (window focused)", { type: request.type, session: request.sessionId })
		return
	}

	// Suppression: cooldown per session+type
	const key = `${request.sessionId}:${request.type}`
	const last = lastShown.get(key) ?? 0
	if (Date.now() - last < COOLDOWN_MS) {
		log.debug("Suppressed (cooldown)", { key })
		return
	}

	// Batch permissions for the same session
	if (request.type === "permission") {
		batchPermissionNotification(request)
		return
	}

	fireNotification(request)
}

/**
 * Dismiss any active notification for a session.
 * Called when the user navigates to the session in the UI.
 */
export function dismissNotification(sessionId: string): void {
	const notification = activeNotifications.get(sessionId)
	if (notification) {
		notification.close()
		activeNotifications.delete(sessionId)
	}
}

/**
 * Update the dock badge count (macOS) or app badge (Linux/Windows).
 */
export function updateBadgeCount(count: number): void {
	const prefs = getNotificationSettings()
	if (!prefs.dockBadge) {
		// Clear any existing badge when disabled
		if (process.platform === "darwin") {
			app.dock?.setBadge("")
		} else {
			app.setBadgeCount(0)
		}
		return
	}
	if (process.platform === "darwin") {
		app.dock?.setBadge(count > 0 ? String(count) : "")
	} else {
		app.setBadgeCount(count)
	}
}

// ============================================================
// Internal
// ============================================================

function fireNotification(request: NotificationRequest): void {
	const key = `${request.sessionId}:${request.type}`
	lastShown.set(key, Date.now())

	// Close any existing notification for this session
	activeNotifications.get(request.sessionId)?.close()

	// Build notification options — add action buttons for permissions on macOS.
	// Actions require: (1) signed app, (2) NSUserNotificationAlertStyle=alert in Info.plist.
	// On unsigned dev builds, actions are silently ignored (no crash).
	const isMac = process.platform === "darwin"
	const hasPermissionActions =
		isMac && request.type === "permission" && request.meta?.permissionId && permissionResponder

	const notification = new Notification({
		title: request.title,
		body: request.body,
		silent: false,
		...(hasPermissionActions && {
			actions: [
				{ type: "button" as const, text: "Allow" },
				{ type: "button" as const, text: "Deny" },
			],
		}),
	})

	notification.on("click", () => {
		navigateToSession(request.sessionId)
		activeNotifications.delete(request.sessionId)
	})

	notification.on("close", () => {
		activeNotifications.delete(request.sessionId)
	})

	// macOS action button handler — Approve or Deny directly from the notification
	if (hasPermissionActions) {
		notification.on("action", (_event, index) => {
			const permissionId = request.meta?.permissionId
			if (!permissionId || !permissionResponder) return

			// index 0 = "Allow", index 1 = "Deny"
			const response = index === 0 ? "once" : "reject"
			permissionResponder({
				sessionId: request.sessionId,
				permissionId,
				response,
				directory: request.directory,
			})
		})
	}

	activeNotifications.set(request.sessionId, notification)
	notification.show()

	// Platform attention signals for blocking events
	if (request.type === "permission" || request.type === "question") {
		bounceDock()
		flashWindow()
	}

	log.info("Notification shown", { type: request.type, session: request.sessionId })
}

/** Navigate the renderer to a specific session (used on notification click). */
function navigateToSession(sessionId: string): void {
	const win = BrowserWindow.getAllWindows()[0]
	if (win) {
		if (win.isMinimized()) win.restore()
		win.focus()
		win.webContents.send("notification:navigate", { sessionId })
	}
}

function batchPermissionNotification(request: NotificationRequest): void {
	const queue = batchQueue.get(request.sessionId) ?? []
	queue.push(request)
	batchQueue.set(request.sessionId, queue)

	// Reset the batch timer
	const existingTimer = batchTimers.get(request.sessionId)
	if (existingTimer) clearTimeout(existingTimer)

	batchTimers.set(
		request.sessionId,
		setTimeout(() => {
			const batched = batchQueue.get(request.sessionId) ?? []
			batchQueue.delete(request.sessionId)
			batchTimers.delete(request.sessionId)

			if (batched.length === 0) return
			if (batched.length === 1) {
				fireNotification(batched[0])
				return
			}

			fireNotification({
				type: "permission",
				sessionId: request.sessionId,
				title: "Agent needs permissions",
				body: `${batched.length} pending approvals`,
				meta: batched[0].meta,
			})
		}, BATCH_WINDOW_MS),
	)
}

function bounceDock(): void {
	if (process.platform !== "darwin") return
	const win = BrowserWindow.getAllWindows()[0]
	if (win && !win.isFocused()) {
		app.dock?.bounce("informational")
	}
}

function flashWindow(): void {
	const win = BrowserWindow.getAllWindows()[0]
	if (!win || win.isFocused()) return
	win.flashFrame(true)
	win.once("focus", () => win.flashFrame(false))
}
