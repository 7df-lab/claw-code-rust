import { contextBridge, ipcRenderer } from "electron"

/**
 * Preload bridge — exposes a typed API from the main process to the renderer.
 *
 * The renderer accesses these via `window.devo.*`.
 * All methods return Promises (backed by `ipcRenderer.invoke`).
 */
contextBridge.exposeInMainWorld("devo", {
	/** The host platform: "darwin", "win32", or "linux". */
	platform: process.platform,

	/** Returns app version and dev/production mode. */
	getAppInfo: () => ipcRenderer.invoke("app:info"),

	appMenu: {
		popup: (id: "file" | "edit" | "view" | "window", position?: { x: number; y: number }) =>
			ipcRenderer.invoke("app-menu:popup", { id, ...position }),
		onAction: (callback: (action: "new-agent" | "open-folder" | "new-terminal") => void) => {
			const listener = (_event: unknown, action: "new-agent" | "open-folder" | "new-terminal") =>
				callback(action)
			ipcRenderer.on("app-menu:action", listener)
			return () => {
				ipcRenderer.removeListener("app-menu:action", listener)
			}
		},
	},

	// --- Window chrome / liquid glass ---

	/**
	 * Subscribes to the window chrome tier notification from the main process.
	 * Fired once after the window finishes loading.
	 * Tier values: "liquid-glass" | "vibrancy" | "transparent" | "opaque"
	 */
	onChromeTier: (callback: (tier: string) => void) => {
		const listener = (_event: unknown, tier: string) => callback(tier)
		ipcRenderer.on("chrome-tier", listener)
		return () => {
			ipcRenderer.removeListener("chrome-tier", listener)
		}
	},

	/** Get the current chrome tier (pull-based, avoids race with push event). */
	getChromeTier: () => ipcRenderer.invoke("chrome-tier:get"),

	/** Ensures the Devo server is running. Spawns it if not. */
	ensureDevo: () => ipcRenderer.invoke("devo:ensure"),

	/** Gets the URL of the running server, or null. */
	getServerUrl: () => ipcRenderer.invoke("devo:url"),

	/** Stops the managed Devo server. */
	stopDevo: () => ipcRenderer.invoke("devo:stop"),

	/** Restarts the managed Devo server (stops and re-starts with current settings). */
	restartDevo: () => ipcRenderer.invoke("devo:restart"),

	onTerminalToggle: (callback: () => void) => {
		const listener = () => callback()
		ipcRenderer.on("terminal:toggle", listener)
		return () => {
			ipcRenderer.removeListener("terminal:toggle", listener)
		}
	},

	native: {
		request: (request: { method: string; params?: unknown; directory?: string }) =>
			ipcRenderer.invoke("native:request", request),
		notify: (notification: { method: string; params?: unknown; directory?: string }) =>
			ipcRenderer.invoke("native:notify", notification),
		respond: (response: { id: number | string; result: unknown }) =>
			ipcRenderer.invoke("native:respond", response),
		connected: () => ipcRenderer.invoke("native:connected"),
		subscribe: (callback: (event: unknown) => void) => {
			const listener = (_event: unknown, value: unknown) => callback(value)
			ipcRenderer.on("native:event", listener)
			return () => {
				ipcRenderer.removeListener("native:event", listener)
			}
		},
	},

	nativeTraffic: {
		getState: () => ipcRenderer.invoke("native-traffic-log:state"),
	},

	terminal: {
		create: (options: { cwd?: string; cols?: number; rows?: number }) =>
			ipcRenderer.invoke("terminal:create", options),
		write: (id: string, data: string) => ipcRenderer.send("terminal:write", id, data),
		resize: (id: string, cols: number, rows: number) =>
			ipcRenderer.send("terminal:resize", id, cols, rows),
		close: (id: string) => ipcRenderer.invoke("terminal:close", id),
		onData: (callback: (event: { id: string; data: string }) => void) => {
			const listener = (_event: unknown, value: { id: string; data: string }) => callback(value)
			ipcRenderer.on("terminal:data", listener)
			return () => {
				ipcRenderer.removeListener("terminal:data", listener)
			}
		},
		onExit: (
			callback: (event: { id: string; exitCode: number; signal?: number }) => void,
		) => {
			const listener = (
				_event: unknown,
				value: { id: string; exitCode: number; signal?: number },
			) => callback(value)
			ipcRenderer.on("terminal:exit", listener)
			return () => {
				ipcRenderer.removeListener("terminal:exit", listener)
			}
		},
	},

	// --- Credential storage (safeStorage-backed) ---

	credential: {
		store: (serverId: string, password: string) =>
			ipcRenderer.invoke("credential:store", serverId, password),
		get: (serverId: string) => ipcRenderer.invoke("credential:get", serverId),
		delete: (serverId: string) => ipcRenderer.invoke("credential:delete", serverId),
	},

	/** Reads model state (recent models, favorites, variants). */
	getModelState: () => ipcRenderer.invoke("model-state"),

	/** Updates the recent model list (adds model to front, deduplicates, caps at 10). */
	updateModelRecent: (model: { providerID: string; modelID: string }) =>
		ipcRenderer.invoke("model-state:update-recent", model),

	// --- Auto-updater ---

	/** Gets the current auto-updater state. */
	getUpdateState: () => ipcRenderer.invoke("updater:state"),

	/** Manually triggers an update check. */
	checkForUpdates: () => ipcRenderer.invoke("updater:check"),

	/** Starts downloading the available update. */
	downloadUpdate: () => ipcRenderer.invoke("updater:download"),

	/** Quits the app and installs the downloaded update. */
	installUpdate: () => ipcRenderer.invoke("updater:install"),

	/** Opens the GitHub release page for the current update version. */
	openReleasePage: () => ipcRenderer.invoke("updater:open-release-page"),

	/** Subscribes to update state changes pushed from the main process. */
	onUpdateStateChanged: (callback: (state: unknown) => void) => {
		const listener = (_event: unknown, state: unknown) => callback(state)
		ipcRenderer.on("updater:state-changed", listener)
		return () => {
			ipcRenderer.removeListener("updater:state-changed", listener)
		}
	},

	// --- Git operations ---

	git: {
		listBranches: (directory: string) => ipcRenderer.invoke("git:branches", directory),
		getStatus: (directory: string) => ipcRenderer.invoke("git:status", directory),
		checkout: (directory: string, branch: string) =>
			ipcRenderer.invoke("git:checkout", directory, branch),
		stashAndCheckout: (directory: string, branch: string) =>
			ipcRenderer.invoke("git:stash-and-checkout", directory, branch),
		stashPop: (directory: string) => ipcRenderer.invoke("git:stash-pop", directory),
		getRoot: (directory: string) => ipcRenderer.invoke("git:root", directory),
		diffStat: (directory: string) => ipcRenderer.invoke("git:diff-stat", directory),
		commitAll: (directory: string, message: string) =>
			ipcRenderer.invoke("git:commit-all", directory, message),
		push: (directory: string, remote?: string) => ipcRenderer.invoke("git:push", directory, remote),
		createBranch: (directory: string, branchName: string) =>
			ipcRenderer.invoke("git:create-branch", directory, branchName),
		applyToLocal: (worktreeDir: string, localDir: string) =>
			ipcRenderer.invoke("git:apply-to-local", worktreeDir, localDir),
		applyDiffText: (localDir: string, diffText: string) =>
			ipcRenderer.invoke("git:apply-diff-text", localDir, diffText),
		getRemoteUrl: (directory: string, remote?: string) =>
			ipcRenderer.invoke("git:remote-url", directory, remote),
	},

	// --- Window preferences (opaque windows / transparency) ---

	/** Get the persisted opaque windows preference from the main process. */
	getOpaqueWindows: () => ipcRenderer.invoke("prefs:get-opaque-windows"),

	/** Set the opaque windows preference and persist it in the main process. */
	setOpaqueWindows: (value: boolean) => ipcRenderer.invoke("prefs:set-opaque-windows", value),

	/** Relaunch the app (used after toggling transparency, which requires a restart). */
	relaunch: () => ipcRenderer.invoke("app:relaunch"),

	// --- Open in external app ---

	openIn: {
		getTargets: () => ipcRenderer.invoke("open-in:targets"),
		open: (directory: string, targetId: string, persistPreferred?: boolean) =>
			ipcRenderer.invoke("open-in:open", directory, targetId, persistPreferred),
		setPreferred: (targetId: string) => ipcRenderer.invoke("open-in:set-preferred", targetId),
	},

	// --- Native theme (syncs OS chrome to app color scheme) ---

	/** Set the native theme source to control OS chrome tint and symbols. */
	setNativeTheme: (source: string) => ipcRenderer.invoke("theme:set-native", source),

	/** Get the system accent color as an 8-char hex RRGGBBAA string, or null if unavailable. */
	getAccentColor: () => ipcRenderer.invoke("theme:accent-color"),

	/** Subscribe to system accent color changes (fired when the user changes OS accent color). */
	onAccentColorChanged: (callback: (color: string) => void) => {
		const listener = (_event: unknown, color: string) => callback(color)
		ipcRenderer.on("theme:accent-color-changed", listener)
		return () => {
			ipcRenderer.removeListener("theme:accent-color-changed", listener)
		}
	},

	// --- Directory picker ---

	/** Opens a native folder picker dialog. Returns the selected path, or null if cancelled. */
	pickDirectory: () => ipcRenderer.invoke("dialog:open-directory"),

	rules: {
		list: (directories: string[]) => ipcRenderer.invoke("rules:list", directories),
		open: (filePath: string) => ipcRenderer.invoke("rules:open", filePath),
		create: (directory: string) => ipcRenderer.invoke("rules:create", directory),
	},

	desktopFolders: {
		stat: (directories: string[]) => ipcRenderer.invoke("desktop-folders:stat", directories),
		create: (input: { parentDirectory: string; name: string }) =>
			ipcRenderer.invoke("desktop-folders:create", input),
	},

	// --- Fetch proxy (bypasses Chromium connection limits) ---

	/**
	 * Proxies an HTTP request through the main process using Electron's `net.fetch()`.
	 * This bypasses Chromium's 6-connections-per-origin limit for HTTP/1.1.
	 * The renderer serializes the Request, sends it over IPC, and gets back
	 * a serialized Response.
	 */
	fetch: (req: {
		url: string
		method: string
		headers: Record<string, string>
		body: string | null
	}) => ipcRenderer.invoke("fetch:request", req),

	// --- Notifications ---

	/**
	 * Subscribes to notification navigation events from the main process.
	 * Fired when the user clicks a native OS notification — the renderer
	 * should navigate to the specified session.
	 */
	onNotificationNavigate: (callback: (data: { sessionId: string }) => void) => {
		const listener = (_event: unknown, data: { sessionId: string }) => callback(data)
		ipcRenderer.on("notification:navigate", listener)
		return () => {
			ipcRenderer.removeListener("notification:navigate", listener)
		}
	},

	/** Subscribe to tray New Chat requests from the main process. */
	onTrayNewChat: (callback: () => void) => {
		const listener = () => callback()
		ipcRenderer.on("tray:new-chat", listener)
		return () => {
			ipcRenderer.removeListener("tray:new-chat", listener)
		}
	},

	/** Dismiss any active notification for a session (e.g. when the user navigates to it). */
	dismissNotification: (sessionId: string) => ipcRenderer.invoke("notification:dismiss", sessionId),

	/** Update the dock badge / app badge count. */
	updateBadgeCount: (count: number) => ipcRenderer.invoke("notification:badge", count),

	// --- Settings ---

	/** Get the full app settings object. */
	getSettings: () => ipcRenderer.invoke("settings:get"),

	/** Update settings with a partial object (deep-merged). */
	updateSettings: (partial: Record<string, unknown>) =>
		ipcRenderer.invoke("settings:update", partial),

	/** Subscribe to settings changes pushed from the main process. */
	onSettingsChanged: (callback: (settings: unknown) => void) => {
		const listener = (_event: unknown, settings: unknown) => callback(settings)
		ipcRenderer.on("settings:changed", listener)
		return () => {
			ipcRenderer.removeListener("settings:changed", listener)
		}
	},

	// --- Automations ---

	automation: {
		list: () => ipcRenderer.invoke("automation:list"),
		get: (id: string) => ipcRenderer.invoke("automation:get", id),
		create: (input: unknown) => ipcRenderer.invoke("automation:create", input),
		update: (input: unknown) => ipcRenderer.invoke("automation:update", input),
		delete: (id: string) => ipcRenderer.invoke("automation:delete", id),
		runNow: (id: string) => ipcRenderer.invoke("automation:run-now", id),
		listRuns: (automationId?: string) => ipcRenderer.invoke("automation:list-runs", automationId),
		archiveRun: (runId: string) => ipcRenderer.invoke("automation:archive-run", runId),
		acceptRun: (runId: string) => ipcRenderer.invoke("automation:accept-run", runId),
		markRunRead: (runId: string) => ipcRenderer.invoke("automation:mark-run-read", runId),
		previewSchedule: (rrule: string, timezone: string) =>
			ipcRenderer.invoke("automation:preview-schedule", rrule, timezone),
	},

	onAutomationRunsUpdated: (callback: () => void) => {
		const listener = () => callback()
		ipcRenderer.on("automation:runs-updated", listener)
		return () => {
			ipcRenderer.removeListener("automation:runs-updated", listener)
		}
	},

	// --- Onboarding ---

	onboarding: {
		/** Check if the bundled Devo runtime is installed and compatible. */
		checkDevo: () => ipcRenderer.invoke("onboarding:check-devo"),
		/** Quick detect all supported providers (Claude Code, Cursor, Devo). */
		detectProviders: () => ipcRenderer.invoke("onboarding:detect-providers"),
		/** Full scan of a specific provider's configuration. */
		scanProvider: (provider: string) => ipcRenderer.invoke("onboarding:scan-provider", provider),
		/** Dry-run migration preview for a provider. */
		previewMigration: (provider: string, scanResult: unknown, categories: string[]) =>
			ipcRenderer.invoke("onboarding:preview-migration", provider, scanResult, categories),
		/** Execute migration (writes files with backup). */
		executeMigration: (provider: string, scanResult: unknown, categories: string[]) =>
			ipcRenderer.invoke("onboarding:execute-migration", provider, scanResult, categories),
		/** Subscribe to migration progress updates (history writing). */
		onMigrationProgress: (callback: (progress: unknown) => void) => {
			const listener = (_event: unknown, progress: unknown) => callback(progress)
			ipcRenderer.on("onboarding:migration-progress", listener)
			return () => {
				ipcRenderer.removeListener("onboarding:migration-progress", listener)
			}
		},
		/** Restore the most recent migration backup. */
		restoreBackup: () => ipcRenderer.invoke("onboarding:restore-backup"),
	},
})
