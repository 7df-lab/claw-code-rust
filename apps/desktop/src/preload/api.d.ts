/**
 * Type definitions for the Electron preload bridge.
 *
 * These types are shared between the preload script and the renderer.
 * The renderer accesses these via `window.devo`.
 */

export interface DevoServerInfo {
	url: string
	transport: "stdio"
	pid: number | null
	managed: boolean
}

export interface NativeTrafficLogState {
	enabled: boolean
	path: string | null
}

export interface ModelRef {
	providerID: string
	modelID: string
}

export interface ModelState {
	recent: ModelRef[]
	favorite: ModelRef[]
	variant: Record<string, string | undefined>
}

export interface UpdateState {
	status: "idle" | "checking" | "available" | "downloading" | "ready" | "error"
	version?: string
	releaseNotes?: string
	progress?: {
		percent: number
		bytesPerSecond: number
		transferred: number
		total: number
	}
	error?: string
	/** Whether the app can auto-install updates (false on unsigned macOS builds). */
	canAutoInstall: boolean
}

export type AppMenuId = "file" | "edit" | "view" | "window"
export type AppMenuAction = "new-agent" | "open-folder" | "new-terminal"

export interface RuleFileInfo {
	path: string
	name: string
	directory: string
	scope: "user" | "project"
}

export interface AppMenuPosition {
	x: number
	y: number
}

// ============================================================
// Git types
// ============================================================

export type GitBranchState = "branch" | "detached" | "missing" | "not_directory" | "not_git"

export interface GitBranchInfo {
	state: GitBranchState
	current: string
	detached: boolean
	local: string[]
	remote: string[]
}

export interface GitStatusInfo {
	isClean: boolean
	staged: number
	modified: number
	untracked: number
	conflicted: number
	summary: string
}

export interface GitCheckoutResult {
	success: boolean
	error?: string
}

export interface GitStashResult {
	success: boolean
	stashed: boolean
	error?: string
}

export interface GitDiffStat {
	filesChanged: number
	insertions: number
	deletions: number
	files: { path: string; insertions: number; deletions: number }[]
}

export interface GitCommitResult {
	success: boolean
	commitHash?: string
	error?: string
}

export interface GitPushResult {
	success: boolean
	error?: string
}

export interface GitApplyResult {
	success: boolean
	filesApplied: string[]
	error?: string
}

// ============================================================
// Open-in-targets types
// ============================================================

export interface OpenInTarget {
	id: string
	label: string
	available: boolean
	/** Base64-encoded PNG icon data URL, resolved at runtime from the installed app. */
	iconDataUrl?: string
}

export interface OpenInTargetsResult {
	targets: OpenInTarget[]
	availableTargets: string[]
	preferredTarget: string | null
}

// ============================================================
// Server config types (shared between main process and renderer)
// ============================================================

/** Built-in local server, auto-managed by Devo via DevoManager. */
export interface LocalServerConfig {
	id: "local"
	name: string
	type: "local"
}

export type ServerConfig = LocalServerConfig

/** The default built-in local server entry (defined in server-config.ts). */
export declare const DEFAULT_LOCAL_SERVER: LocalServerConfig

export type NetworkProxyMode = "system" | "custom" | "off"

export interface NetworkProxySettings {
	mode: NetworkProxyMode
	proxyUrl: string
	noProxy: string
}

export interface ServerSettings {
	/** Ordered list of configured servers. The local server is always first. */
	servers: ServerConfig[]
	/** ID of the currently active server. */
	activeServerId: string
	/** Network proxy settings for the Desktop-managed private runtime. */
	networkProxy: NetworkProxySettings
}

// ============================================================
// Settings types (shared between main process and renderer)
// ============================================================

export type CompletionNotificationMode = "off" | "unfocused" | "always"
export type ColorScheme = "dark" | "light" | "system"
export type DisplayMode = "default" | "verbose"

export interface NotificationSettings {
	completionMode: CompletionNotificationMode
	permissions: boolean
	questions: boolean
	errors: boolean
	dockBadge: boolean
}

export interface AppearanceSettings {
	colorScheme: ColorScheme
	themeId: string
	displayMode: DisplayMode
	/** Hide model reasoning/thinking blocks while the agent is still working. */
	hideThinkingWhileWorking: boolean
	rendererPreferencesMigrated: boolean
}

export interface OpenInSettings {
	preferredTargetId: string | null
}

export type DesktopFolderStatus = "available" | "missing" | "not_directory"

export interface DesktopFolder {
	id: string
	directory: string
	name?: string
	addedAt: number
}

export interface DesktopFolderSettings {
	folders: DesktopFolder[]
}

export interface DesktopFolderStat {
	directory: string
	status: DesktopFolderStatus
}

export interface CreateDesktopFolderInput {
	parentDirectory: string
	name: string
}

export interface CreateDesktopFolderResult {
	directory: string
	name: string
}

export interface AppSettings {
	notifications: NotificationSettings
	/** Whether the user prefers opaque (solid) windows. Read at window creation time. */
	opaqueWindows: boolean
	/** Desktop UI appearance preferences. */
	appearance: AppearanceSettings
	/** External app target preferences for opening projects. */
	openIn: OpenInSettings
	/** User-managed folders shown in Devo Desktop. */
	desktopFolders: DesktopFolderSettings
	/** Server connection configuration. */
	servers: ServerSettings
}

// ============================================================
// Onboarding types
// ============================================================

export interface DevoCheckResult {
	installed: boolean
	version: string | null
	path: string | null
	compatible: boolean
	compatibility: "ok" | "too-old" | "too-new" | "blocked" | "unknown"
	message: string | null
}

/** Supported migration source providers. */
export type MigrationProvider = "claude-code" | "cursor" | "devo" | "opencode"

/** Detection result for a single provider. */
export interface ProviderDetection {
	provider: MigrationProvider
	found: boolean
	label: string
	summary: string
	mcpServerCount: number
	agentCount: number
	commandCount: number
	ruleCount: number
	skillCount: number
	projectCount: number
	hasGlobalSettings: boolean
	hasPermissions: boolean
	hasHooks: boolean
	totalSessions: number
	totalMessages: number
}

export interface MigrationCategoryPreview {
	category: string
	itemCount: number
	files: MigrationFilePreview[]
}

export interface MigrationFilePreview {
	path: string
	status: "new" | "modified" | "skipped"
	lineCount: number
	content?: string
}

export interface MigrationPreview {
	categories: MigrationCategoryPreview[]
	warnings: string[]
	manualActions: string[]
	errors: string[]
	fileCount: number
	sessionCount: number
	sessionProjectCount: number
}

export interface MigrationResult {
	success: boolean
	filesWritten: string[]
	filesSkipped: string[]
	backupDir: string | null
	warnings: string[]
	manualActions: string[]
	errors: string[]
	/** Number of history sessions that were skipped as duplicates */
	historyDuplicatesSkipped: number
}

export interface MigrationProgress {
	phase: string
	current: number
	total: number
	duplicatesSkipped: number
}

export interface AppInfo {
	version: string
	isDev: boolean
}

export type WindowChromeTier = "liquid-glass" | "vibrancy" | "transparent" | "opaque"

export interface TerminalSessionInfo {
	id: string
	cwd: string
	shell: string
	cols: number
	rows: number
}

export interface TerminalDataEvent {
	id: string
	data: string
}

export interface TerminalExitEvent {
	id: string
	exitCode: number
	signal?: number
}

// ============================================================
// Automation types
// ============================================================

export interface AutomationSchedule {
	rrule: string
	timezone: string
}

export type PermissionPreset = "default" | "allow-all" | "read-only"

export interface ExecutionConfig {
	/** Model to use in "providerID/modelID" format (e.g. "anthropic/claude-opus-4-5"). Defaults to server default. */
	model?: string
	/** Agent name to use (e.g. "build", "research"). Defaults to server default agent. */
	agent?: string
	/** Model variant name (e.g. "extended" for extended thinking). Defaults to model default. */
	variant?: string
	effort: "low" | "medium" | "high"
	timeout: number
	retries: number
	retryDelay: number
	parallelWorkspaces: boolean
	approvalPolicy: "never" | "auto-edit"
	/** Whether to run in an isolated git worktree (default: true) */
	useWorktree: boolean
	/** Permission preset controlling agent tool access */
	permissionPreset: PermissionPreset
}

export type AutomationStatus = "active" | "paused" | "archived"

export interface Automation {
	id: string
	name: string
	prompt: string
	status: AutomationStatus
	schedule: AutomationSchedule
	workspaces: string[]
	execution: ExecutionConfig
	nextRunAt: number | null
	lastRunAt: number | null
	runCount: number
	consecutiveFailures: number
	createdAt: number
	updatedAt: number
}

export type AutomationRunStatus =
	| "queued"
	| "running"
	| "pending_review"
	| "accepted"
	| "archived"
	| "failed"

export interface AutomationRun {
	id: string
	automationId: string
	workspace: string
	status: AutomationRunStatus
	attempt: number
	sessionId: string | null
	worktreePath: string | null
	startedAt: number | null
	completedAt: number | null
	timeoutAt: number | null
	resultTitle: string | null
	resultSummary: string | null
	resultHasActionable: boolean | null
	resultBranch: string | null
	resultPrUrl: string | null
	errorMessage: string | null
	archivedReason: string | null
	archivedAssistantMessage: string | null
	readAt: number | null
	createdAt: number
	updatedAt: number
}

export interface CreateAutomationInput {
	name: string
	prompt: string
	schedule: { rrule: string; timezone?: string }
	workspaces: string[]
	execution?: Partial<ExecutionConfig>
}

export interface UpdateAutomationInput {
	id: string
	name?: string
	prompt?: string
	status?: AutomationStatus
	schedule?: { rrule: string; timezone?: string }
	workspaces?: string[]
	execution?: Partial<ExecutionConfig>
}

export interface DevoAPI {
	/** The host platform: "darwin", "win32", or "linux". */
	platform: NodeJS.Platform
	getAppInfo: () => Promise<AppInfo>
	appMenu: {
		popup: (id: AppMenuId, position?: AppMenuPosition) => Promise<{ success: boolean }>
		onAction: (callback: (action: AppMenuAction) => void) => () => void
	}

	/** Subscribe to chrome tier notification (fired once on load). */
	onChromeTier: (callback: (tier: WindowChromeTier) => void) => () => void
	/** Get the current chrome tier (pull-based, avoids race with push event). */
	getChromeTier: () => Promise<WindowChromeTier>

	ensureDevo: () => Promise<DevoServerInfo>
	getServerUrl: () => Promise<string | null>
	stopDevo: () => Promise<boolean>
	restartDevo: () => Promise<DevoServerInfo>
	onTerminalToggle: (callback: () => void) => () => void
	native: {
		request: (request: {
			method: string
			params?: unknown
			directory?: string
		}) => Promise<unknown>
		notify: (notification: {
			method: string
			params?: unknown
			directory?: string
		}) => Promise<void>
		respond: (response: { id: number | string; result: unknown }) => Promise<void>
		connected: () => Promise<boolean>
		subscribe: (callback: (event: unknown) => void) => () => void
	}
	nativeTraffic: {
		getState: () => Promise<NativeTrafficLogState>
	}
	terminal: {
		create: (options: { cwd?: string; cols?: number; rows?: number }) => Promise<TerminalSessionInfo>
		write: (id: string, data: string) => void
		resize: (id: string, cols: number, rows: number) => void
		close: (id: string) => Promise<void>
		onData: (callback: (event: TerminalDataEvent) => void) => () => void
		onExit: (callback: (event: TerminalExitEvent) => void) => () => void
	}
	getModelState: () => Promise<ModelState>
	updateModelRecent: (model: ModelRef) => Promise<ModelState>

	// Credential storage (safeStorage-backed, passwords never leave main process in plain text)
	credential: {
		/** Store an encrypted password for a server. */
		store: (serverId: string, password: string) => Promise<void>
		/** Retrieve a decrypted password for a server (only returns to renderer for auth headers). */
		get: (serverId: string) => Promise<string | null>
		/** Delete a stored password. */
		delete: (serverId: string) => Promise<void>
	}

	// Auto-updater
	getUpdateState: () => Promise<UpdateState>
	checkForUpdates: () => Promise<void>
	downloadUpdate: () => Promise<void>
	installUpdate: () => Promise<void>
	/** Opens the GitHub release page for the current update version (fallback for unsigned macOS). */
	openReleasePage: () => Promise<void>
	onUpdateStateChanged: (callback: (state: UpdateState) => void) => () => void

	// Git operations
	git: {
		listBranches: (directory: string) => Promise<GitBranchInfo>
		getStatus: (directory: string) => Promise<GitStatusInfo>
		checkout: (directory: string, branch: string) => Promise<GitCheckoutResult>
		stashAndCheckout: (directory: string, branch: string) => Promise<GitStashResult>
		stashPop: (directory: string) => Promise<GitStashResult>
		getRoot: (directory: string) => Promise<string | null>
		diffStat: (directory: string) => Promise<GitDiffStat>
		workspaceChangesSummary: (
			directory: string,
			scope: "uncommitted" | "staged" | "unstaged" | "branch",
			options?: { baseBranch?: string | null; ignoreWhitespace?: boolean },
		) => Promise<{
			scope: string
			status: string
			workspace_root: string
			coverage: string
			attribution: string
			change_set_status: string
			files: Array<{
				path: string
				status: string
				additions: number | null
				deletions: number | null
				binary: boolean
				diff_truncated: boolean
			}>
			stats: { files_changed: number; additions: number; deletions: number }
			unified_diff: string | null
			warnings: string[]
			generated_at: string
		}>
		workspaceFilePatch: (
			directory: string,
			scope: "uncommitted" | "staged" | "unstaged" | "branch" | "turn",
			filePath: string,
			options?: {
				baseBranch?: string | null
				ignoreWhitespace?: boolean
				fileStatus?: string | null
				checkpointId?: string | null
				mergeBase?: string | null
			},
		) => Promise<string>
		commitAll: (directory: string, message: string) => Promise<GitCommitResult>
		push: (directory: string, remote?: string) => Promise<GitPushResult>
		createBranch: (directory: string, branchName: string) => Promise<GitCheckoutResult>
		applyToLocal: (worktreeDir: string, localDir: string) => Promise<GitApplyResult>
		applyDiffText: (localDir: string, diffText: string) => Promise<GitApplyResult>
		getRemoteUrl: (directory: string, remote?: string) => Promise<string | null>
	}

	// Window preferences (opaque windows / transparency)
	/** Get the persisted opaque windows preference from the main process. */
	getOpaqueWindows: () => Promise<boolean>
	/** Set the opaque windows preference and persist it in the main process. */
	setOpaqueWindows: (value: boolean) => Promise<{ success: boolean }>
	/** Relaunch the app (used after toggling transparency). */
	relaunch: () => Promise<void>

	// Open in external app
	openIn: {
		getTargets: () => Promise<OpenInTargetsResult>
		open: (directory: string, targetId: string, persistPreferred?: boolean) => Promise<void>
		setPreferred: (targetId: string) => Promise<{ success: boolean }>
		openMcpConfig?: () => Promise<{ path: string }>
	}

	// Native theme (syncs OS chrome to app color scheme)
	/** Set the native theme source ("light" | "dark" | "system") to control OS chrome tint and symbols. */
	setNativeTheme: (source: string) => Promise<void>

	// System accent color
	/** Get the system accent color as an 8-char hex RRGGBBAA string, or null if unavailable. */
	getAccentColor: () => Promise<string | null>
	/** Subscribe to system accent color changes. Returns an unsubscribe function. */
	onAccentColorChanged: (callback: (color: string) => void) => () => void

	// Directory picker
	pickDirectory: () => Promise<string | null>
	rules: {
		list: (directories: string[]) => Promise<RuleFileInfo[]>
		open: (filePath: string) => Promise<void>
		create: (directory: string) => Promise<RuleFileInfo>
	}
	mcp?: {
		openConfig: () => Promise<{ path: string }>
	}
	desktopFolders: {
		stat: (directories: string[]) => Promise<DesktopFolderStat[]>
		create: (input: CreateDesktopFolderInput) => Promise<CreateDesktopFolderResult>
	}

	// Fetch proxy (bypasses Chromium connection limits)
	fetch: (req: {
		url: string
		method: string
		headers: Record<string, string>
		body: string | null
	}) => Promise<{
		status: number
		statusText: string
		headers: Record<string, string>
		body: string | null
	}>

	// Notifications
	/** Subscribe to navigation events from native OS notification clicks. */
	onNotificationNavigate: (callback: (data: { sessionId: string }) => void) => () => void
	/** Subscribe to native tray New Chat requests. */
	onTrayNewChat: (callback: () => void) => () => void
	/** Dismiss any active notification for a session. */
	dismissNotification: (sessionId: string) => Promise<void>
	/** Update the dock badge / app badge count. */
	updateBadgeCount: (count: number) => Promise<void>

	// Settings
	/** Get the full app settings object. */
	getSettings: () => Promise<AppSettings>
	/** Update settings with a partial object (deep-merged). Returns the updated settings. */
	updateSettings: (partial: Record<string, unknown>) => Promise<AppSettings>
	/** Subscribe to settings changes pushed from the main process. */
	onSettingsChanged: (callback: (settings: AppSettings) => void) => () => void

	// Automations
	automation: {
		list: () => Promise<Automation[]>
		get: (id: string) => Promise<Automation | null>
		create: (input: CreateAutomationInput) => Promise<Automation>
		update: (input: UpdateAutomationInput) => Promise<Automation | null>
		delete: (id: string) => Promise<boolean>
		runNow: (id: string) => Promise<boolean>
		listRuns: (automationId?: string) => Promise<AutomationRun[]>
		archiveRun: (runId: string) => Promise<boolean>
		acceptRun: (runId: string) => Promise<boolean>
		markRunRead: (runId: string) => Promise<boolean>
		previewSchedule: (rrule: string, timezone: string) => Promise<string[]>
	}
	/** Subscribe to automation run state changes. */
	onAutomationRunsUpdated: (callback: () => void) => () => void

	onboarding: {
		checkDevo: () => Promise<DevoCheckResult>
		/** Quick-detect all supported providers (Claude Code, Cursor, Devo, OpenCode). */
		detectProviders: () => Promise<ProviderDetection[]>
		/** Full scan of a specific provider's configuration. */
		scanProvider: (
			provider: MigrationProvider,
		) => Promise<{ detection: ProviderDetection; scanResult: unknown }>
		/** Dry-run migration preview for a provider. */
		previewMigration: (
			provider: MigrationProvider,
			scanResult: unknown,
			categories: string[],
		) => Promise<MigrationPreview>
		/** Execute migration (writes files with backup). */
		executeMigration: (
			provider: MigrationProvider,
			scanResult: unknown,
			categories: string[],
		) => Promise<MigrationResult>
		/** Subscribe to migration progress updates (history writing). */
		onMigrationProgress: (callback: (progress: MigrationProgress) => void) => () => void
		/** Restore the most recent migration backup. */
		restoreBackup: () => Promise<{
			success: boolean
			restored: string[]
			removed: string[]
			errors: string[]
		}>
	}
}

declare global {
	interface Window {
		devo: DevoAPI
	}
}
