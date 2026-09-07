/**
 * Unified backend service layer.
 *
 * Detects whether we're running inside Electron (preload bridge available)
 * or in a plain browser (Bun + Hono server on port 3100). All hooks import
 * from here instead of `devo-server.ts` directly.
 *
 * In Electron mode, calls go through IPC to the main process.
 * In browser mode, calls go through HTTP to the Devo server.
 */

import type {
	Automation,
	AutomationRun,
	CreateAutomationInput,
	CreateDesktopFolderInput,
	CreateDesktopFolderResult,
	DesktopFolderStat,
	GitApplyResult,
	GitBranchInfo,
	GitCheckoutResult,
	GitCommitResult,
	GitDiffStat,
	GitPushResult,
	GitStashResult,
	GitStatusInfo,
	ModelState,
	OpenInTargetsResult,
	UpdateAutomationInput,
} from "../../preload/api"
import { MCP_CONFIG_OPEN_PATH } from "../../shared/mcp-config"
import { createLogger } from "../lib/logger"

const log = createLogger("backend")

// ============================================================
// Runtime detection
// ============================================================

/**
 * Returns true when running inside Electron (preload bridge is available).
 * The `devo` object is exposed via `contextBridge.exposeInMainWorld`.
 */
export const isElectron = typeof window !== "undefined" && "devo" in window

/** Prefer runtime checks — module-load `isElectron` can be wrong before preload binds. */
export function canUseLocalWorkspaceGit(): boolean {
	try {
		return (
			typeof window !== "undefined" &&
			typeof window.devo?.git?.workspaceChangesSummary === "function" &&
			typeof window.devo?.git?.workspaceFilePatch === "function"
		)
	} catch {
		return false
	}
}

// ============================================================
// Backend API — same signatures regardless of runtime
// ============================================================

/**
 * Ensures the single local Devo stdio runtime is running and returns its
 * compatibility URL.
 */
export async function fetchDevoUrl(): Promise<{ url: string }> {
	log.debug("fetchDevoUrl", { via: isElectron ? "ipc" : "http" })
	try {
		if (isElectron) {
			const info = await window.devo.ensureDevo()
			log.info("Devo server URL resolved", { url: info.url })
			return { url: info.url }
		}
		const { fetchDevoUrl: httpFetch } = await import("./devo-server")
		const result = await httpFetch()
		log.info("Devo server URL resolved", { url: result.url })
		return result
	} catch (err) {
		log.error("fetchDevoUrl failed", err)
		throw err
	}
}

/**
 * Resolve the connection URL for the local server config.
 */
export async function resolveServerUrl(
	server: import("../../preload/api").ServerConfig,
): Promise<string> {
	if (server.type !== "local") throw new Error("Remote Devo servers are disabled in this build")
	const { url } = await fetchDevoUrl()
	return url
}

/**
 * The local stdio transport does not use HTTP auth.
 */
export async function resolveAuthHeader(
	_server: import("../../preload/api").ServerConfig,
): Promise<string | null> {
	return null
}

/**
 * Fetches the Devo model state (recent models, favorites, variants)
 * from ~/.local/state/devo/model.json.
 */
export async function fetchModelState(): Promise<ModelState> {
	if (isElectron) {
		return window.devo.getModelState()
	}
	const { fetchModelState: httpFetch } = await import("./devo-server")
	return httpFetch() as unknown as Promise<ModelState>
}

/**
 * Adds a model to the front of the recent list in model.json.
 * Matches the TUI's `model.set(model, { recent: true })` behavior.
 * Returns the updated model state.
 */
export async function updateModelRecent(model: {
	providerID: string
	modelID: string
}): Promise<ModelState> {
	if (isElectron) {
		return window.devo.updateModelRecent(model)
	}
	const { updateModelRecent: httpUpdate } = await import("./devo-server")
	return httpUpdate(model) as unknown as Promise<ModelState>
}

/**
 * Checks if the backend is available.
 * In Electron, always returns true (main process is always there).
 * In browser, pings the Devo HTTP server.
 */
export async function checkBackendHealth(): Promise<boolean> {
	if (isElectron) {
		return true
	}
	const { checkServerHealth } = await import("./devo-server")
	return checkServerHealth()
}

// ============================================================
// Directory picker — Electron-only (native dialog via IPC)
// ============================================================

/**
 * Opens a native folder picker dialog.
 * Returns the selected directory path, or null if cancelled.
 */
export async function pickDirectory(): Promise<string | null> {
	if (isElectron) {
		return window.devo.pickDirectory()
	}
	throw new Error("Directory picker is only available in Electron mode")
}

export async function statDesktopFolders(directories: string[]): Promise<DesktopFolderStat[]> {
	if (isElectron && window.devo.desktopFolders?.stat) {
		return window.devo.desktopFolders.stat(directories)
	}
	return directories.map((directory) => ({ directory, status: "available" }))
}

export async function createDesktopFolder(
	input: CreateDesktopFolderInput,
): Promise<CreateDesktopFolderResult> {
	if (isElectron && window.devo.desktopFolders?.create) {
		return window.devo.desktopFolders.create(input)
	}
	throw new Error("Folder creation requires the updated desktop bridge. Restart Devo Desktop and try again.")
}

// ============================================================
// Git operations — Electron-only (main process via IPC)
// In browser mode, these are not available (Devo server
// doesn't expose git checkout/stash APIs).
// ============================================================

/**
 * Lists all local and remote branches for a project directory.
 */
export async function fetchGitBranches(directory: string): Promise<GitBranchInfo> {
	if (isElectron) {
		return window.devo.git.listBranches(directory)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Gets the working tree status (clean/dirty, file counts).
 */
export async function fetchGitStatus(directory: string): Promise<GitStatusInfo> {
	if (isElectron) {
		return window.devo.git.getStatus(directory)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Checks out a branch. Fails if there are uncommitted changes
 * that would conflict.
 */
export async function gitCheckout(directory: string, branch: string): Promise<GitCheckoutResult> {
	if (isElectron) {
		return window.devo.git.checkout(directory, branch)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Stashes uncommitted changes, then checks out the target branch.
 */
export async function gitStashAndCheckout(
	directory: string,
	branch: string,
): Promise<GitStashResult> {
	if (isElectron) {
		return window.devo.git.stashAndCheckout(directory, branch)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Pops the most recent stash entry.
 */
export async function gitStashPop(directory: string): Promise<GitStashResult> {
	if (isElectron) {
		return window.devo.git.stashPop(directory)
	}
	throw new Error("Git operations are only available in Electron mode")
}

// ============================================================
// Worktree operations — Devo API only
// ============================================================

export type { WorktreeResult } from "./worktree-service"
export {
	createWorktree as createWorktreeViaApi,
	listWorktrees as listWorktreesViaApi,
	removeWorktree as removeWorktreeViaApi,
	resetWorktree,
} from "./worktree-service"

/**
 * Gets the git repository root for a directory.
 */
export async function getGitRoot(directory: string): Promise<string | null> {
	if (isElectron) {
		return window.devo.git.getRoot(directory)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Gets a summary of uncommitted changes in a directory.
 */
export async function fetchDiffStat(directory: string): Promise<GitDiffStat> {
	if (isElectron) {
		return window.devo.git.diffStat(directory)
	}
	throw new Error("Git operations are only available in Electron mode")
}

export type LocalGitChangeScope = "uncommitted" | "staged" | "unstaged" | "branch"

export type LocalWorkspaceFilePatchOptions = {
	baseBranch?: string | null
	ignoreWhitespace?: boolean
	fileStatus?: string | null
	checkpointId?: string | null
	mergeBase?: string | null
}

/** Local Summary list for working-tree scopes (bypasses Devo stdio). */
export async function fetchLocalWorkspaceChangesSummary(
	directory: string,
	scope: LocalGitChangeScope,
	options?: { baseBranch?: string | null; ignoreWhitespace?: boolean },
) {
	if (!canUseLocalWorkspaceGit()) {
		throw new Error("Local workspace changes require Electron git IPC")
	}
	return window.devo.git.workspaceChangesSummary(directory, scope, options)
}

/** Single-file patch for expand-on-demand (bypasses whole-tree Full). */
export async function fetchLocalWorkspaceFilePatch(
	directory: string,
	scope: LocalGitChangeScope | "turn",
	filePath: string,
	options?: LocalWorkspaceFilePatchOptions,
) {
	if (!canUseLocalWorkspaceGit()) {
		throw new Error("Local workspace file patch requires Electron git IPC")
	}
	return window.devo.git.workspaceFilePatch(directory, scope, filePath, options)
}

/**
 * Commits all changes (staged + unstaged) with the given message.
 */
export async function gitCommitAll(directory: string, message: string): Promise<GitCommitResult> {
	if (isElectron) {
		return window.devo.git.commitAll(directory, message)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Pushes the current branch to the remote.
 */
export async function gitPush(directory: string, remote?: string): Promise<GitPushResult> {
	if (isElectron) {
		return window.devo.git.push(directory, remote)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Creates a new branch on the given directory.
 */
export async function gitCreateBranch(
	directory: string,
	branchName: string,
): Promise<GitCheckoutResult> {
	if (isElectron) {
		return window.devo.git.createBranch(directory, branchName)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Gets the remote URL for a repository (defaults to "origin").
 */
export async function getGitRemoteUrl(directory: string, remote?: string): Promise<string | null> {
	if (isElectron) {
		return window.devo.git.getRemoteUrl(directory, remote)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Applies uncommitted changes from a worktree to the local checkout as a patch.
 */
export async function gitApplyToLocal(
	worktreeDir: string,
	localDir: string,
): Promise<GitApplyResult> {
	if (isElectron) {
		return window.devo.git.applyToLocal(worktreeDir, localDir)
	}
	throw new Error("Git operations are only available in Electron mode")
}

/**
 * Applies a raw diff string to a local directory using `git apply`.
 * Used for remote worktree apply-to-local, where the diff is fetched
 * from the Devo session.diff API rather than from a local worktree.
 */
export async function gitApplyDiffText(
	localDir: string,
	diffText: string,
): Promise<GitApplyResult> {
	if (isElectron) {
		return window.devo.git.applyDiffText(localDir, diffText)
	}
	throw new Error("Git operations are only available in Electron mode")
}

// ============================================================
// Open in external app — Electron-only (main process via IPC)
// ============================================================

/**
 * Gets the list of available "Open in" targets (editors, terminals, file managers)
 * with their availability status and the user's preferred target.
 */
export async function fetchOpenInTargets(): Promise<OpenInTargetsResult> {
	if (isElectron) {
		return window.devo.openIn.getTargets()
	}
	throw new Error("Open-in targets are only available in Electron mode")
}

/**
 * Opens a directory in the specified target application.
 * Optionally persists the target as the user's preferred choice.
 */
export async function openInTarget(
	directory: string,
	targetId: string,
	persistPreferred?: boolean,
): Promise<void> {
	if (isElectron) {
		return window.devo.openIn.open(directory, targetId, persistPreferred)
	}
	throw new Error("Open-in targets are only available in Electron mode")
}

/**
 * Sets the user's preferred "Open in" target without opening anything.
 */
export async function setOpenInPreferred(targetId: string): Promise<{ success: boolean }> {
	if (isElectron) {
		return window.devo.openIn.setPreferred(targetId)
	}
	throw new Error("Open-in targets are only available in Electron mode")
}

/**
 * Ensures the user MCP config file exists and opens it with the General
 * settings "Default open destination" app.
 *
 * Prefers dedicated preload methods when present. Falls back to the existing
 * `openIn.open` bridge so this still works if the window was created before
 * a newer `mcp` preload namespace was added.
 */
export async function openMcpConfigFile(): Promise<{ path?: string }> {
	if (typeof window === "undefined" || !("devo" in window)) {
		throw new Error("MCP config can only be edited in the desktop app")
	}
	if (typeof window.devo.mcp?.openConfig === "function") {
		return window.devo.mcp.openConfig()
	}
	if (typeof window.devo.openIn.openMcpConfig === "function") {
		return window.devo.openIn.openMcpConfig()
	}
	const { preferredTarget, availableTargets } = await window.devo.openIn.getTargets()
	const targetId = preferredTarget ?? availableTargets[0]
	if (!targetId) {
		throw new Error("No app is available to open the MCP config file")
	}
	await window.devo.openIn.open(MCP_CONFIG_OPEN_PATH, targetId)
	return {}
}

// ============================================================
// Automations — Electron-only
// ============================================================

export async function fetchAutomations(): Promise<Automation[]> {
	if (isElectron) {
		return window.devo.automation.list()
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function fetchAutomation(id: string): Promise<Automation | null> {
	if (isElectron) {
		return window.devo.automation.get(id)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function createAutomation(input: CreateAutomationInput): Promise<Automation> {
	if (isElectron) {
		return window.devo.automation.create(input)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function updateAutomation(input: UpdateAutomationInput): Promise<Automation | null> {
	if (isElectron) {
		return window.devo.automation.update(input)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function deleteAutomation(id: string): Promise<boolean> {
	if (isElectron) {
		return window.devo.automation.delete(id)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function runAutomationNow(id: string): Promise<boolean> {
	if (isElectron) {
		return window.devo.automation.runNow(id)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function fetchAutomationRuns(automationId?: string): Promise<AutomationRun[]> {
	if (isElectron) {
		return window.devo.automation.listRuns(automationId)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function archiveAutomationRun(runId: string): Promise<boolean> {
	if (isElectron) {
		return window.devo.automation.archiveRun(runId)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function acceptAutomationRun(runId: string): Promise<boolean> {
	if (isElectron) {
		return window.devo.automation.acceptRun(runId)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function markAutomationRunRead(runId: string): Promise<boolean> {
	if (isElectron) {
		return window.devo.automation.markRunRead(runId)
	}
	throw new Error("Automations are only available in Electron mode")
}

export async function previewAutomationSchedule(
	rrule: string,
	timezone: string,
): Promise<string[]> {
	if (isElectron) {
		return window.devo.automation.previewSchedule(rrule, timezone)
	}
	throw new Error("Automations are only available in Electron mode")
}
