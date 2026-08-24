import { app, BrowserWindow, dialog, ipcMain, nativeTheme, net, shell, systemPreferences } from "electron"
import {
	acceptRun,
	archiveRun,
	createAutomation,
	deleteAutomation,
	getAutomation,
	listAutomations,
	listRuns,
	markRunRead,
	previewSchedule,
	runNow,
	updateAutomation,
} from "./automation"
import type { CreateAutomationInput, UpdateAutomationInput } from "./automation/types"
import { deleteCredential, getCredential, storeCredential } from "./credential-store"
import { checkDesktopRuntime } from "./desktop-runtime-check"
import { createDesktopFolder, statDesktopFolders } from "./desktop-folders"
import {
	applyChangesToLocal,
	applyDiffTextToLocal,
	checkout,
	commitAll,
	createBranch,
	getDiffStat,
	getGitRoot,
	getRemoteUrl,
	getStatus,
	listBranches,
	push,
	stashAndCheckout,
	stashPop,
} from "./git-service"
import { getResolvedChromeTier, resolveTitleBarOverlay } from "./liquid-glass"
import { createProjectAgentsMd, listRuleFiles } from "./rules-files"
import { createLogger } from "./logger"
import { readModelState, updateModelRecent } from "./model-state"
import { dismissNotification, updateBadgeCount } from "./notifications"
import type { MigrationProvider } from "./onboarding"
import {
	detectProviders,
	executeMigration,
	previewMigration,
	restoreMigrationBackup,
	scanProvider,
} from "./onboarding"
import { openUserMcpConfigFile } from "./mcp-config"
import { getOpenInTargets, openInTarget, setPreferredTarget } from "./open-in-targets"
import { MCP_CONFIG_OPEN_PATH } from "../shared/mcp-config"
import {
	ensureServer,
	getNativeTrafficLogState,
	getServerUrl,
	isNativeConnected,
	notifyNative,
	requestNative,
	respondNative,
	restartServer,
	stopServer,
	subscribeNative,
} from "./devo-manager"
import { getOpaqueWindows, getSettings, onSettingsChanged, updateSettings } from "./settings-store"
import { desktopTerminalManager } from "./terminal-manager"
import {
	checkForUpdates,
	downloadUpdate,
	getUpdateState,
	installUpdate,
	openReleasePage,
} from "./updater"

const log = createLogger("ipc")

/** Read the opaque windows preference for use at window creation time. */
export { getOpaqueWindows as getOpaqueWindowsPref } from "./settings-store"

function updateTitleBarOverlay(): void {
	if (process.platform !== "win32" && process.platform !== "linux") return
	const titleBarOverlay = resolveTitleBarOverlay(nativeTheme.shouldUseDarkColors)
	for (const win of BrowserWindow.getAllWindows()) {
		win.setTitleBarOverlay(titleBarOverlay)
	}
}

// ============================================================
// Serialized fetch types — used to pass Request/Response over IPC
// ============================================================

interface SerializedRequest {
	url: string
	method: string
	headers: Record<string, string>
	body: string | null
}

interface SerializedResponse {
	status: number
	statusText: string
	headers: Record<string, string>
	body: string | null
}

/**
 * Generic fetch proxy handler for the renderer process.
 *
 * The renderer serializes a Request into a plain object, sends it over IPC,
 * and the main process performs the actual HTTP request using `net.fetch()`
 * (Electron's network stack, which has no connection-per-origin limits).
 * The response is serialized back to the renderer.
 *
 * This bypasses Chromium's 6-connections-per-origin HTTP/1.1 limit, which
 * causes severe queueing when many parallel requests hit the Devo server.
 */
async function handleFetchProxy(
	_event: Electron.IpcMainInvokeEvent,
	req: SerializedRequest,
): Promise<SerializedResponse> {
	log.info("IPC fetch proxy →", { method: req.method, url: req.url })
	const start = Date.now()
	const response = await net.fetch(req.url, {
		method: req.method,
		headers: req.headers,
		body: req.body ?? undefined,
	})

	const body = await response.text()
	const headers: Record<string, string> = {}
	response.headers.forEach((value, key) => {
		headers[key] = value
	})
	const durationMs = Date.now() - start

	log.info("IPC fetch proxy ←", {
		method: req.method,
		url: req.url,
		status: response.status,
		bodyLength: body.length,
		durationMs,
	})

	return {
		status: response.status,
		statusText: response.statusText,
		headers,
		body,
	}
}

/**
 * Wraps an IPC handler to log errors before they propagate to the renderer.
 * Without this, errors thrown in handlers are silently serialized across IPC
 * and the main process log shows nothing.
 */
function withLogging<TArgs extends unknown[], TResult>(
	channel: string,
	handler: (...args: TArgs) => TResult | Promise<TResult>,
): (...args: TArgs) => Promise<TResult> {
	return async (...args: TArgs) => {
		const start = Date.now()
		try {
			const result = await handler(...args)
			const durationMs = Date.now() - start
			if (durationMs > 500) {
				log.warn(`Handler "${channel}" slow`, { durationMs })
			}
			return result
		} catch (err) {
			log.error(`Handler "${channel}" failed`, { durationMs: Date.now() - start }, err)
			throw err
		}
	}
}

/**
 * Registers all IPC handlers that the renderer can invoke via contextBridge.
 *
 * Each handler corresponds to an endpoint that was previously served by
 * the Bun + Hono server on port 3100. Now they run in-process in Electron's
 * main process, communicating via IPC instead of HTTP.
 */
export function registerIpcHandlers(): void {
	// --- App info ---

	ipcMain.handle("app:info", () => ({
		version: app.getVersion(),
		isDev: !app.isPackaged,
	}))

	// --- Devo server lifecycle ---

	ipcMain.handle(
		"devo:ensure",
		withLogging("devo:ensure", async () => await ensureServer()),
	)

	ipcMain.handle("devo:url", () => getServerUrl())

	ipcMain.handle(
		"devo:stop",
		withLogging("devo:stop", () => stopServer()),
	)

	ipcMain.handle(
		"devo:restart",
		withLogging("devo:restart", async () => await restartServer()),
	)

	ipcMain.handle(
		"native:request",
		withLogging(
			"native:request",
			async (_, request: { method: string; params?: unknown; directory?: string }) =>
				await requestNative(request.method, request.params, request.directory),
		),
	)

	ipcMain.handle(
		"native:notify",
		withLogging(
			"native:notify",
			async (_, notification: { method: string; params?: unknown; directory?: string }) =>
				await notifyNative(notification.method, notification.params, notification.directory),
		),
	)

	ipcMain.handle(
		"native:respond",
		withLogging(
			"native:respond",
			async (_, response: { id: number | string; result: unknown }) =>
				await respondNative(response.id, response.result),
		),
	)

	ipcMain.handle("native:connected", () => isNativeConnected())

	ipcMain.handle("native-traffic-log:state", () => getNativeTrafficLogState())

	subscribeNative((event) => {
		for (const win of BrowserWindow.getAllWindows()) {
			win.webContents.send("native:event", event)
		}
	})

	// --- Embedded terminal ---

	desktopTerminalManager.onData((id, data) => {
		for (const win of BrowserWindow.getAllWindows()) {
			win.webContents.send("terminal:data", { id, data })
		}
	})

	desktopTerminalManager.onExit((id, event) => {
		for (const win of BrowserWindow.getAllWindows()) {
			win.webContents.send("terminal:exit", { id, ...event })
		}
	})

	ipcMain.handle(
		"terminal:create",
		withLogging(
			"terminal:create",
			async (_, options: { cwd?: string; cols?: number; rows?: number }) =>
				await desktopTerminalManager.create(options),
		),
	)

	ipcMain.handle("terminal:close", (_, id: string) => {
		desktopTerminalManager.close(id)
	})

	ipcMain.on("terminal:write", (_, id: string, data: string) => {
		desktopTerminalManager.write(id, data)
	})

	ipcMain.on("terminal:resize", (_, id: string, cols: number, rows: number) => {
		desktopTerminalManager.resize(id, cols, rows)
	})

	// --- Model state ---

	ipcMain.handle(
		"model-state",
		withLogging("model-state", async () => await readModelState()),
	)

	ipcMain.handle(
		"model-state:update-recent",
		withLogging(
			"model-state:update-recent",
			async (_, model: { providerID: string; modelID: string }) => await updateModelRecent(model),
		),
	)

	// --- Auto-updater ---

	ipcMain.handle("updater:state", () => getUpdateState())

	ipcMain.handle("updater:check", async () => await checkForUpdates())

	ipcMain.handle("updater:download", async () => await downloadUpdate())

	ipcMain.handle("updater:install", async () => await installUpdate())

	ipcMain.handle("updater:open-release-page", async () => await openReleasePage())

	// --- Git operations ---

	ipcMain.handle(
		"git:branches",
		withLogging("git:branches", async (_, directory: string) => await listBranches(directory)),
	)

	ipcMain.handle(
		"git:status",
		withLogging("git:status", async (_, directory: string) => await getStatus(directory)),
	)

	ipcMain.handle(
		"git:checkout",
		withLogging(
			"git:checkout",
			async (_, directory: string, branch: string) => await checkout(directory, branch),
		),
	)

	ipcMain.handle(
		"git:stash-and-checkout",
		withLogging(
			"git:stash-and-checkout",
			async (_, directory: string, branch: string) => await stashAndCheckout(directory, branch),
		),
	)

	ipcMain.handle(
		"git:stash-pop",
		withLogging("git:stash-pop", async (_, directory: string) => await stashPop(directory)),
	)

	ipcMain.handle(
		"git:diff-stat",
		withLogging("git:diff-stat", async (_, directory: string) => await getDiffStat(directory)),
	)

	ipcMain.handle(
		"git:commit-all",
		withLogging(
			"git:commit-all",
			async (_, directory: string, message: string) => await commitAll(directory, message),
		),
	)

	ipcMain.handle(
		"git:push",
		withLogging(
			"git:push",
			async (_, directory: string, remote?: string) => await push(directory, remote),
		),
	)

	ipcMain.handle(
		"git:create-branch",
		withLogging(
			"git:create-branch",
			async (_, directory: string, branchName: string) => await createBranch(directory, branchName),
		),
	)

	ipcMain.handle(
		"git:apply-to-local",
		withLogging(
			"git:apply-to-local",
			async (_, worktreeDir: string, localDir: string) =>
				await applyChangesToLocal(worktreeDir, localDir),
		),
	)

	ipcMain.handle(
		"git:apply-diff-text",
		withLogging(
			"git:apply-diff-text",
			async (_, localDir: string, diffText: string) =>
				await applyDiffTextToLocal(localDir, diffText),
		),
	)

	ipcMain.handle(
		"git:root",
		withLogging("git:root", async (_, directory: string) => await getGitRoot(directory)),
	)

	ipcMain.handle(
		"git:remote-url",
		withLogging(
			"git:remote-url",
			async (_, directory: string, remote?: string) => await getRemoteUrl(directory, remote),
		),
	)

	// --- Directory picker ---

	ipcMain.handle(
		"dialog:open-directory",
		withLogging("dialog:open-directory", async () => {
			const result = await dialog.showOpenDialog({
				properties: ["openDirectory"],
				title: "Select a project folder",
			})
			if (result.canceled || result.filePaths.length === 0) return null
			return result.filePaths[0]
		}),
	)

	ipcMain.handle(
		"desktop-folders:stat",
		withLogging(
			"desktop-folders:stat",
			async (_, directories: string[]) => await statDesktopFolders(directories),
		),
	)

	ipcMain.handle(
		"desktop-folders:create",
		withLogging(
			"desktop-folders:create",
			async (_, input: { parentDirectory: string; name: string }) =>
				await createDesktopFolder(input),
		),
	)

	ipcMain.handle(
		"rules:list",
		withLogging("rules:list", (_, directories: string[]) =>
			listRuleFiles(Array.isArray(directories) ? directories : []),
		),
	)

	ipcMain.handle(
		"rules:open",
		withLogging("rules:open", async (_, filePath: string) => {
			const error = await shell.openPath(filePath)
			if (error) throw new Error(error)
		}),
	)

	ipcMain.handle(
		"rules:create",
		withLogging("rules:create", (_, directory: string) => createProjectAgentsMd(directory)),
	)

	ipcMain.handle(
		"mcp:open-config",
		withLogging("mcp:open-config", async () => await openUserMcpConfigFile()),
	)

	// --- Fetch proxy (bypasses Chromium connection limits) ---

	ipcMain.handle("fetch:request", withLogging("fetch:request", handleFetchProxy))

	// --- Open in external app ---

	ipcMain.handle("open-in:targets", () => getOpenInTargets())

	ipcMain.handle(
		"open-in:open",
		withLogging(
			"open-in:open",
			async (_, directory: string, targetId: string, persistPreferred?: boolean) => {
				if (directory === MCP_CONFIG_OPEN_PATH) {
					return await openUserMcpConfigFile()
				}
				await openInTarget(directory, targetId, { persistPreferred })
			},
		),
	)

	ipcMain.handle("open-in:set-preferred", (_, targetId: string) => {
		setPreferredTarget(targetId)
		return { success: true }
	})

	// --- Chrome tier (pull-based, avoids race with push-based "chrome-tier" event) ---

	ipcMain.handle("chrome-tier:get", () => getResolvedChromeTier())

	// --- Window preferences (opaque windows) ---

	ipcMain.handle("prefs:get-opaque-windows", () => {
		return getOpaqueWindows()
	})

	ipcMain.handle("prefs:set-opaque-windows", (_, value: boolean) => {
		updateSettings({ opaqueWindows: value })
		return { success: true }
	})

	ipcMain.handle("app:relaunch", () => {
		app.relaunch()
		app.exit(0)
	})

	// --- Notifications ---

	ipcMain.handle("notification:dismiss", (_, sessionId: string) => {
		dismissNotification(sessionId)
	})

	ipcMain.handle("notification:badge", (_, count: number) => {
		updateBadgeCount(count)
	})

	// --- Settings ---

	ipcMain.handle("settings:get", () => getSettings())

	ipcMain.handle("settings:update", (_, partial) => updateSettings(partial))

	// --- Credential storage (safeStorage-backed) ---

	ipcMain.handle(
		"credential:store",
		withLogging("credential:store", (_, serverId: string, password: string) => {
			storeCredential(serverId, password)
		}),
	)

	ipcMain.handle("credential:get", (_, serverId: string) => getCredential(serverId))

	ipcMain.handle(
		"credential:delete",
		withLogging("credential:delete", (_, serverId: string) => {
			deleteCredential(serverId)
		}),
	)

	// --- Native theme (controls macOS glass tint color) ---

	ipcMain.handle("theme:set-native", (_, source: string) => {
		if (source === "light" || source === "dark") {
			nativeTheme.themeSource = source
		} else {
			nativeTheme.themeSource = "system"
		}
		updateTitleBarOverlay()
	})

	nativeTheme.on("updated", updateTitleBarOverlay)

	// --- System accent color (macOS / Windows) ---

	ipcMain.handle("theme:accent-color", () => {
		try {
			return systemPreferences.getAccentColor()
		} catch {
			return null
		}
	})

	// Broadcast accent color changes to all renderer windows
	systemPreferences.on("accent-color-changed", (_event, newColor) => {
		for (const win of BrowserWindow.getAllWindows()) {
			win.webContents.send("theme:accent-color-changed", newColor)
		}
	})

	// --- Onboarding ---

	ipcMain.handle(
		"onboarding:check-devo",
		withLogging("onboarding:check-devo", async () => await checkDesktopRuntime()),
	)

	ipcMain.handle(
		"onboarding:detect-providers",
		withLogging("onboarding:detect-providers", async () => await detectProviders()),
	)

	ipcMain.handle(
		"onboarding:scan-provider",
		withLogging(
			"onboarding:scan-provider",
			async (_, provider: MigrationProvider) => await scanProvider(provider),
		),
	)

	ipcMain.handle(
		"onboarding:preview-migration",
		withLogging(
			"onboarding:preview-migration",
			async (_, provider: MigrationProvider, scanResult: unknown, categories: string[]) =>
				await previewMigration(provider, scanResult, categories),
		),
	)

	ipcMain.handle(
		"onboarding:execute-migration",
		withLogging(
			"onboarding:execute-migration",
			async (_, provider: MigrationProvider, scanResult: unknown, categories: string[]) =>
				await executeMigration(provider, scanResult, categories),
		),
	)

	ipcMain.handle(
		"onboarding:restore-backup",
		withLogging("onboarding:restore-backup", async () => await restoreMigrationBackup()),
	)

	// --- Automations ---

	ipcMain.handle(
		"automation:list",
		withLogging("automation:list", () => listAutomations()),
	)

	ipcMain.handle(
		"automation:get",
		withLogging("automation:get", (_, id: string) => getAutomation(id)),
	)

	ipcMain.handle(
		"automation:create",
		withLogging("automation:create", async (_, input: CreateAutomationInput) => {
			const result = await createAutomation(input)
			for (const win of BrowserWindow.getAllWindows()) {
				win.webContents.send("automation:runs-updated")
			}
			return result
		}),
	)

	ipcMain.handle(
		"automation:update",
		withLogging("automation:update", async (_, input: UpdateAutomationInput) => {
			const result = await updateAutomation(input)
			for (const win of BrowserWindow.getAllWindows()) {
				win.webContents.send("automation:runs-updated")
			}
			return result
		}),
	)

	ipcMain.handle(
		"automation:delete",
		withLogging("automation:delete", async (_, id: string) => {
			const result = await deleteAutomation(id)
			for (const win of BrowserWindow.getAllWindows()) {
				win.webContents.send("automation:runs-updated")
			}
			return result
		}),
	)

	ipcMain.handle(
		"automation:run-now",
		withLogging("automation:run-now", async (_, id: string) => {
			// runNow is fire-and-forget: it returns immediately after validating
			// the automation exists. Execution happens in the background, and
			// broadcastRunsUpdated() is called from within executeAutomation.
			return runNow(id)
		}),
	)

	ipcMain.handle(
		"automation:list-runs",
		withLogging("automation:list-runs", (_, automationId?: string) => listRuns(automationId)),
	)

	ipcMain.handle(
		"automation:archive-run",
		withLogging("automation:archive-run", async (_, runId: string) => {
			const result = await archiveRun(runId)
			for (const win of BrowserWindow.getAllWindows()) {
				win.webContents.send("automation:runs-updated")
			}
			return result
		}),
	)

	ipcMain.handle(
		"automation:accept-run",
		withLogging("automation:accept-run", async (_, runId: string) => {
			const result = await acceptRun(runId)
			for (const win of BrowserWindow.getAllWindows()) {
				win.webContents.send("automation:runs-updated")
			}
			return result
		}),
	)

	ipcMain.handle(
		"automation:mark-run-read",
		withLogging("automation:mark-run-read", async (_, runId: string) => {
			const result = await markRunRead(runId)
			for (const win of BrowserWindow.getAllWindows()) {
				win.webContents.send("automation:runs-updated")
			}
			return result
		}),
	)

	ipcMain.handle(
		"automation:preview-schedule",
		withLogging("automation:preview-schedule", (_, rrule: string, timezone: string) =>
			previewSchedule(rrule, timezone),
		),
	)

	// --- Settings push channel (main -> renderer) ---
	// Notify all renderer windows when settings change so they can update reactively.

	onSettingsChanged((settings) => {
		for (const win of BrowserWindow.getAllWindows()) {
			win.webContents.send("settings:changed", settings)
		}
	})
}
