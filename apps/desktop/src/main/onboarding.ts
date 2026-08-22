/**
 * Onboarding handlers for the Devo desktop app.
 *
 * Provides IPC-callable functions for the first-run experience:
 * - Devo CLI detection and version compatibility check
 * - Devo CLI installation (via curl/shell)
 * - Multi-provider config detection and migration via @devo/configconv
 *   Supported providers: Claude Code, Cursor, Devo, OpenCode
 */

import { type ChildProcess, spawn } from "node:child_process"
import { existsSync } from "node:fs"
import { homedir } from "node:os"
import path from "node:path"
import { BrowserWindow } from "electron"
import type { DevoCheckResult } from "./compatibility"
import { checkDevo } from "./compatibility"
import {
	buildClaudeCodeProviderMigrationPreview,
	executeClaudeCodeProviderMigration,
} from "./claude-code-provider-migration"
import {
	buildOpenCodeProviderMigrationPreview,
	executeOpenCodeProviderMigration,
} from "./opencode-provider-migration"
import { requestNative } from "./devo-manager"
import { createLogger } from "./logger"

const log = createLogger("onboarding")

// ============================================================
// Types
// ============================================================

/** Supported migration source providers. */
export type MigrationProvider = "claude-code" | "cursor" | "devo" | "opencode"

/** Quick-detect result for a single provider (no heavy imports). */
export interface ProviderDetection {
	provider: MigrationProvider
	found: boolean
	/** Human-readable label */
	label: string
	/** Short summary of what was found */
	summary: string
	/** Number of MCP servers found */
	mcpServerCount: number
	/** Number of agents found */
	agentCount: number
	/** Number of commands found */
	commandCount: number
	/** Number of rule files found (e.g. .mdc rules, CLAUDE.md, AGENTS.md) */
	ruleCount: number
	/** Number of skills found */
	skillCount: number
	/** Number of projects found */
	projectCount: number
	/** Whether global settings exist */
	hasGlobalSettings: boolean
	/** Whether permissions exist (cli-config.json, settings.json, etc.) */
	hasPermissions: boolean
	/** Claude Code specific: hooks present */
	hasHooks: boolean
	/** Claude Code specific: total sessions for history import */
	totalSessions: number
	/** Claude Code specific: total messages for history import */
	totalMessages: number
}

export interface MigrationPreview {
	categories: MigrationCategoryPreview[]
	warnings: string[]
	manualActions: string[]
	errors: string[]
	fileCount: number
	/** Number of sessions that will be imported (0 if history not selected) */
	sessionCount: number
	/** Number of projects the sessions span */
	sessionProjectCount: number
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

// ============================================================
// Provider metadata
// ============================================================

const PROVIDER_LABELS: Record<MigrationProvider, string> = {
	"claude-code": "Claude Code",
	cursor: "Cursor",
	devo: "Devo",
	opencode: "OpenCode",
}

// ============================================================
// Devo check (delegates to compatibility module)
// ============================================================

export async function checkDevoInstallation(): Promise<DevoCheckResult> {
	return checkDevo()
}

// ============================================================
// Devo install
// ============================================================

let installProcess: ChildProcess | null = null

/**
 * Installs Devo CLI by running the official install script.
 * Streams output lines to the renderer via the "onboarding:install-output" channel.
 * Returns when the install process exits.
 */
export async function installDevo(): Promise<{ success: boolean; error?: string }> {
	if (installProcess) {
		return { success: false, error: "Installation already in progress" }
	}

	return new Promise((resolve) => {
		const isWindows = process.platform === "win32"

		if (isWindows) {
			// Windows: use PowerShell to run the install script
			installProcess = spawn(
				"powershell",
				["-Command", "irm https://devo.ai/install.ps1 | iex"],
				{
					cwd: homedir(),
					stdio: "pipe",
					env: process.env,
				},
			)
		} else {
			// macOS/Linux: use bash + curl
			installProcess = spawn("bash", ["-c", "curl -fsSL https://devo.ai/install | bash"], {
				cwd: homedir(),
				stdio: "pipe",
				env: process.env,
			})
		}

		const proc = installProcess

		const sendOutput = (text: string) => {
			for (const win of BrowserWindow.getAllWindows()) {
				win.webContents.send("onboarding:install-output", text)
			}
		}

		proc.stdout?.on("data", (data: Buffer) => {
			const text = data.toString()
			sendOutput(text)
			log.debug(`[install:stdout] ${text.trim()}`)
		})

		proc.stderr?.on("data", (data: Buffer) => {
			const text = data.toString()
			sendOutput(text)
			log.debug(`[install:stderr] ${text.trim()}`)
		})

		proc.on("error", (err) => {
			log.error("Install process error", err)
			installProcess = null
			resolve({ success: false, error: err.message })
		})

		proc.on("exit", (code) => {
			installProcess = null
			if (code === 0) {
				log.info("Devo install completed successfully")
				resolve({ success: true })
			} else {
				log.warn("Devo install exited with code", code)
				resolve({ success: false, error: `Install script exited with code ${code}` })
			}
		})
	})
}

// ============================================================
// Multi-provider detection (lightweight, no configconv import)
// ============================================================

/**
 * Quickly detects which agent tools have configuration on this machine.
 * Does NOT import @devo/configconv, just checks for file/directory existence.
 * Returns an array of detections (one per supported provider).
 */
export async function detectProviders(): Promise<ProviderDetection[]> {
	const results = await Promise.all([
		detectClaudeCode(),
		detectCursor(),
		detectDevoProvider(),
		detectOpenCode(),
	])
	return results
}

/**
 * Quickly detects whether Claude Code configuration exists on this machine.
 */
async function detectClaudeCode(): Promise<ProviderDetection> {
	const home = homedir()
	const claudeSettingsDirs = [path.join(home, ".claude"), path.join(home, ".Claude")]

	const hasGlobalSettings = claudeSettingsDirs.some((dir) =>
		existsSync(path.join(dir, "settings.json")),
	)
	const hasUserState = existsSync(path.join(home, ".claude.json"))

	// Check for projects directory to estimate project count.
	let projectCount = 0
	for (const settingsDir of claudeSettingsDirs) {
		try {
			const { readdirSync } = await import("node:fs")
			const entries = readdirSync(path.join(settingsDir, "projects"), { withFileTypes: true })
			projectCount = entries.filter((e) => e.isDirectory()).length
			break
		} catch {
			// Try the next Claude Code directory.
		}
	}

	let ruleCount = 0
	if (existsSync(path.join(home, ".claude", "CLAUDE.md"))) ruleCount++
	if (existsSync("CLAUDE.md")) ruleCount++

	const found = hasGlobalSettings || hasUserState || projectCount > 0

	const summaryParts: string[] = []
	if (hasGlobalSettings) summaryParts.push("global settings")
	if (projectCount > 0) summaryParts.push(`${projectCount} project${projectCount === 1 ? "" : "s"}`)
	if (ruleCount > 0) summaryParts.push("rules")

	return {
		provider: "claude-code",
		found,
		label: PROVIDER_LABELS["claude-code"],
		summary: found ? `Found ${summaryParts.join(", ")}` : "No Claude Code configuration detected",
		hasGlobalSettings: hasGlobalSettings || hasUserState,
		hasPermissions: hasGlobalSettings,
		projectCount,
		mcpServerCount: 0,
		agentCount: 0,
		commandCount: 0,
		ruleCount,
		hasHooks: false,
		skillCount: 0,
		totalSessions: 0,
		totalMessages: 0,
	}
}

/**
 * Quickly detects whether Cursor configuration exists on this machine.
 * Reads mcp.json to count servers and checks for rules directories.
 */
async function detectCursor(): Promise<ProviderDetection> {
	const { readFileSync, readdirSync } = await import("node:fs")
	const home = homedir()
	const cursorDir = path.join(home, ".cursor")

	// MCP servers: parse mcp.json to count actual server entries
	let mcpServerCount = 0
	const mcpJsonPath = path.join(cursorDir, "mcp.json")
	if (existsSync(mcpJsonPath)) {
		try {
			const raw = readFileSync(mcpJsonPath, "utf-8")
			const parsed = JSON.parse(raw)
			if (parsed?.mcpServers && typeof parsed.mcpServers === "object") {
				mcpServerCount = Object.keys(parsed.mcpServers).length
			}
		} catch {
			// malformed JSON, count as 0
		}
	}

	// Permissions from cli-config.json
	const hasCliConfig = existsSync(path.join(cursorDir, "cli-config.json"))

	// Skills
	let skillCount = 0
	const skillsDir = path.join(cursorDir, "skills")
	if (existsSync(skillsDir)) {
		try {
			skillCount = readdirSync(skillsDir, { withFileTypes: true }).filter(
				(e) => e.isDirectory() && e.name !== "skills-cursor",
			).length
		} catch {
			// ignore
		}
	}

	// Agents (global)
	let agentCount = 0
	const agentsDir = path.join(cursorDir, "agents")
	if (existsSync(agentsDir)) {
		try {
			agentCount = readdirSync(agentsDir).filter((f) => f.endsWith(".md")).length
		} catch {
			// ignore
		}
	}

	// Commands (global)
	let commandCount = 0
	const commandsDir = path.join(cursorDir, "commands")
	if (existsSync(commandsDir)) {
		try {
			commandCount = readdirSync(commandsDir).filter((f) => f.endsWith(".md")).length
		} catch {
			// ignore
		}
	}

	// Rules: Cursor stores rules per-project in .cursor/rules/*.mdc|*.md.
	// The lightweight detector cannot scan all projects, but it checks
	// if the global cursor dir has any rule-like content.
	// (Full scan via scanProvider will get project-level rule counts.)
	const ruleCount = 0

	const found =
		mcpServerCount > 0 || hasCliConfig || skillCount > 0 || agentCount > 0 || commandCount > 0

	const summaryParts: string[] = []
	if (mcpServerCount > 0)
		summaryParts.push(`${mcpServerCount} MCP server${mcpServerCount === 1 ? "" : "s"}`)
	if (hasCliConfig) summaryParts.push("permissions")
	if (agentCount > 0) summaryParts.push(`${agentCount} agent${agentCount === 1 ? "" : "s"}`)
	if (commandCount > 0) summaryParts.push(`${commandCount} command${commandCount === 1 ? "" : "s"}`)
	if (skillCount > 0) summaryParts.push(`${skillCount} skill${skillCount === 1 ? "" : "s"}`)

	return {
		provider: "cursor",
		found,
		label: PROVIDER_LABELS.cursor,
		summary: found ? `Found ${summaryParts.join(", ")}` : "No Cursor configuration detected",
		hasGlobalSettings: mcpServerCount > 0,
		hasPermissions: hasCliConfig,
		projectCount: 0,
		mcpServerCount,
		agentCount,
		commandCount,
		ruleCount,
		hasHooks: false,
		skillCount,
		totalSessions: 0,
		totalMessages: 0,
	}
}

/**
 * Quickly detects whether OpenCode provider configuration exists on this machine.
 */
async function detectOpenCode(): Promise<ProviderDetection> {
	const { readFileSync } = await import("node:fs")
	const home = homedir()
	const xdgConfig = process.env.XDG_CONFIG_HOME || path.join(home, ".config")
	const opencodeDir = path.join(xdgConfig, "opencode")
	const configPaths = [
		path.join(opencodeDir, "opencode.json"),
		path.join(opencodeDir, "opencode.jsonc"),
	]
	const configPath = configPaths.find(existsSync)
	const found = !!configPath
	let providerCount = 0
	let modelCount = 0

	if (configPath) {
		try {
			const parsed = JSON.parse(readFileSync(configPath, "utf-8"))
			if (parsed?.provider && typeof parsed.provider === "object") {
				const providers = Object.values(parsed.provider) as Array<Record<string, unknown>>
				providerCount = providers.length
				for (const provider of providers) {
					if (provider.models && typeof provider.models === "object") {
						modelCount += Object.keys(provider.models).length
					}
				}
			}
		} catch {
			// JSONC or malformed config; full scan will parse JSONC and report details.
		}
	}

	const summaryParts: string[] = []
	if (providerCount > 0) {
		summaryParts.push(`${providerCount} provider${providerCount === 1 ? "" : "s"}`)
	}
	if (modelCount > 0) {
		summaryParts.push(`${modelCount} model${modelCount === 1 ? "" : "s"}`)
	}
	if (found && summaryParts.length === 0) summaryParts.push("global config")

	return {
		provider: "opencode",
		found,
		label: PROVIDER_LABELS.opencode,
		summary: found ? `Found ${summaryParts.join(", ")}` : "No OpenCode configuration detected",
		hasGlobalSettings: found,
		hasPermissions: false,
		projectCount: 0,
		mcpServerCount: 0,
		agentCount: 0,
		commandCount: 0,
		ruleCount: 0,
		hasHooks: false,
		skillCount: 0,
		totalSessions: 0,
		totalMessages: 0,
	}
}

/**
 * Quickly detects whether Devo configuration exists on this machine.
 * Parses devo.json to count MCP servers.
 */
async function detectDevoProvider(): Promise<ProviderDetection> {
	const { readFileSync, readdirSync } = await import("node:fs")
	const home = homedir()
	const xdgConfig = process.env.XDG_CONFIG_HOME || path.join(home, ".config")
	const ocDir = path.join(xdgConfig, "devo")

	// Parse devo.json to count MCP servers
	let mcpServerCount = 0
	let hasConfig = false
	let hasPermissions = false
	const configPath = path.join(ocDir, "devo.json")
	if (existsSync(configPath)) {
		hasConfig = true
		try {
			const raw = readFileSync(configPath, "utf-8")
			const parsed = JSON.parse(raw)
			if (parsed?.mcp && typeof parsed.mcp === "object") {
				mcpServerCount = Object.keys(parsed.mcp).length
			}
			if (parsed?.permission && typeof parsed.permission === "object") {
				hasPermissions = Object.keys(parsed.permission).length > 0
			}
		} catch {
			// malformed JSON
		}
	}

	const hasAgentsMd = existsSync(path.join(ocDir, "AGENTS.md"))

	let agentCount = 0
	const agentsDir = path.join(ocDir, "agents")
	if (existsSync(agentsDir)) {
		try {
			agentCount = readdirSync(agentsDir).filter((f) => f.endsWith(".md")).length
		} catch {
			// ignore
		}
	}

	let commandCount = 0
	const commandsDir = path.join(ocDir, "commands")
	if (existsSync(commandsDir)) {
		try {
			commandCount = readdirSync(commandsDir).filter((f) => f.endsWith(".md")).length
		} catch {
			// ignore
		}
	}

	let skillCount = 0
	const skillsDir = path.join(ocDir, "skills")
	if (existsSync(skillsDir)) {
		try {
			skillCount = readdirSync(skillsDir, { withFileTypes: true }).filter((e) =>
				e.isDirectory(),
			).length
		} catch {
			// ignore
		}
	}

	const ruleCount = hasAgentsMd ? 1 : 0
	const found = hasConfig || hasAgentsMd || agentCount > 0 || commandCount > 0

	const summaryParts: string[] = []
	if (hasConfig) summaryParts.push("global config")
	if (mcpServerCount > 0)
		summaryParts.push(`${mcpServerCount} MCP server${mcpServerCount === 1 ? "" : "s"}`)
	if (hasAgentsMd) summaryParts.push("rules")
	if (agentCount > 0) summaryParts.push(`${agentCount} agent${agentCount === 1 ? "" : "s"}`)
	if (commandCount > 0) summaryParts.push(`${commandCount} command${commandCount === 1 ? "" : "s"}`)
	if (skillCount > 0) summaryParts.push(`${skillCount} skill${skillCount === 1 ? "" : "s"}`)

	return {
		provider: "devo",
		found,
		label: PROVIDER_LABELS.devo,
		summary: found ? `Found ${summaryParts.join(", ")}` : "No Devo configuration detected",
		hasGlobalSettings: hasConfig,
		hasPermissions,
		projectCount: 0,
		mcpServerCount,
		agentCount,
		commandCount,
		ruleCount,
		hasHooks: false,
		skillCount,
		totalSessions: 0,
		totalMessages: 0,
	}
}

// ============================================================
// Migration (lazy-loads @devo/configconv)
// ============================================================

/**
 * Runs a full scan for the specified provider and returns detailed detection results.
 * Lazy-loads @devo/configconv to keep the main process fast when not needed.
 */
export async function scanProvider(provider: MigrationProvider): Promise<{
	detection: ProviderDetection
	scanResult: unknown
}> {
	const { scanFormat } = await import("@devo/configconv")

	const scanResult = await scanFormat({
		format: provider,
		global: true,
		includeHistory: provider === "claude-code" || provider === "cursor",
	})

	const detection = buildDetectionFromScan(provider, scanResult)
	return { detection, scanResult }
}

/**
 * Runs a dry-run migration preview. Returns what would be changed without writing anything.
 */
export async function previewMigration(
	provider: MigrationProvider,
	scanResult: unknown,
	categories: string[],
): Promise<MigrationPreview> {
	if (provider === "opencode") {
		const providerPreview = categories.includes("config")
			? buildOpenCodeProviderMigrationPreview(scanResult)
			: { category: null, warnings: [], manualActions: [], errors: [] }
		const categoryPreviews = providerPreview.category ? [providerPreview.category] : []
		const totalFiles = categoryPreviews.reduce((sum, category) => sum + category.files.length, 0)
		return {
			categories: categoryPreviews,
			warnings: providerPreview.warnings,
			manualActions: providerPreview.manualActions,
			errors: providerPreview.errors,
			fileCount: totalFiles,
			sessionCount: 0,
			sessionProjectCount: 0,
		}
	}

	const { universalConvert } = await import("@devo/configconv")

	// Convert from source provider to Devo (the target for Devo)
	// biome-ignore lint/suspicious/noExplicitAny: scanResult is dynamically typed from IPC
	const conversion = universalConvert(scanResult as any, { to: "devo" })

	const categoryPreviews: MigrationCategoryPreview[] = []
	const claudeCodeProviderPreview =
		provider === "claude-code" && categories.includes("config")
			? buildClaudeCodeProviderMigrationPreview(scanResult)
			: null

	// Global config
	if (Object.keys(conversion.globalConfig).length > 0) {
		const content = JSON.stringify(conversion.globalConfig, null, 2)
		categoryPreviews.push({
			category: "config",
			itemCount: 1,
			files: [
				{
					path: "~/.config/devo/devo.json",
					status: "new",
					lineCount: content.split("\n").length,
					content,
				},
			],
		})
	}

	// Project configs
	for (const [projectPath, config] of conversion.projectConfigs) {
		if (Object.keys(config).length > 0) {
			const content = JSON.stringify(config, null, 2)
			categoryPreviews.push({
				category: "mcp",
				itemCount: 1,
				files: [
					{
						path: path.join(projectPath, "devo.json"),
						status: "new",
						lineCount: content.split("\n").length,
						content,
					},
				],
			})
		}
	}

	// Agents
	if (conversion.agents.size > 0) {
		const files: MigrationFilePreview[] = []
		for (const [filePath, content] of conversion.agents) {
			files.push({
				path: filePath,
				status: "new",
				lineCount: content.split("\n").length,
				content,
			})
		}
		categoryPreviews.push({ category: "agents", itemCount: files.length, files })
	}

	// Commands
	if (conversion.commands.size > 0) {
		const files: MigrationFilePreview[] = []
		for (const [filePath, content] of conversion.commands) {
			files.push({
				path: filePath,
				status: "new",
				lineCount: content.split("\n").length,
				content,
			})
		}
		categoryPreviews.push({ category: "commands", itemCount: files.length, files })
	}

	// Rules
	if (conversion.rules.size > 0) {
		const files: MigrationFilePreview[] = []
		for (const [filePath, content] of conversion.rules) {
			files.push({
				path: filePath,
				status: "new",
				lineCount: content.split("\n").length,
				content,
			})
		}
		categoryPreviews.push({ category: "rules", itemCount: files.length, files })
	}

	// Extra files (plugins etc.)
	if (conversion.extraFiles.size > 0) {
		const files: MigrationFilePreview[] = []
		for (const [filePath, content] of conversion.extraFiles) {
			files.push({
				path: filePath,
				status: "new",
				lineCount: content.split("\n").length,
				content,
			})
		}
		categoryPreviews.push({ category: "extra", itemCount: files.length, files })
	}

	if (claudeCodeProviderPreview?.category) {
		categoryPreviews.push(claudeCodeProviderPreview.category)
	}

	// History (Cursor or Claude Code)
	let sessionCount = 0
	let sessionProjectCount = 0
	if (categories.includes("history")) {
		const historyResult = await previewHistoryMigration(provider, scanResult)
		if (historyResult) {
			sessionCount = historyResult.sessionCount
			sessionProjectCount = historyResult.projectCount
			if (sessionCount > 0) {
				categoryPreviews.push({
					category: "history",
					itemCount: sessionCount,
					files: [
						{
							path: "~/.local/share/devo/storage/",
							status: "new",
							lineCount: 0,
							content: `${sessionCount} chat sessions across ${sessionProjectCount} projects will be imported`,
						},
					],
				})
			}
		}
	}

	const totalFiles = categoryPreviews.reduce((sum, c) => sum + c.files.length, 0)

	return {
		categories: categoryPreviews,
		warnings: [...conversion.report.warnings, ...(claudeCodeProviderPreview?.warnings ?? [])],
		manualActions: [
			...conversion.report.manualActions,
			...(claudeCodeProviderPreview?.manualActions ?? []),
		],
		errors: [...conversion.report.errors, ...(claudeCodeProviderPreview?.errors ?? [])],
		fileCount: totalFiles,
		sessionCount,
		sessionProjectCount,
	}
}

/**
 * Preview history migration for the given provider scan result.
 */
async function previewHistoryMigration(
	provider: MigrationProvider,
	scanResult: unknown,
): Promise<{ sessionCount: number; projectCount: number } | null> {
	// biome-ignore lint/suspicious/noExplicitAny: dynamically typed from IPC
	const result = scanResult as any
	if (provider === "cursor" && result?.data?.history) {
		const history = result.data.history
		const projectPaths = new Set<string>()
		for (const session of history.sessions ?? []) {
			if (session.projectPath) projectPaths.add(session.projectPath)
		}
		return {
			sessionCount: history.totalSessions ?? 0,
			projectCount: projectPaths.size,
		}
	}
	if (provider === "claude-code" && result?.data?.history) {
		return {
			sessionCount: result.data.history.totalSessions ?? 0,
			projectCount: result.data.history.sessionIndices?.length ?? 0,
		}
	}
	return null
}

/**
 * Executes the migration, writing files to disk with a backup.
 * Sends progress events to the renderer via IPC during history migration.
 */
export async function executeMigration(
	provider: MigrationProvider,
	scanResult: unknown,
	categories: string[],
): Promise<MigrationResult> {
	if (provider === "opencode") {
		const providerResult = categories.includes("config")
			? await executeOpenCodeProviderMigration(scanResult, requestNative)
			: { filesWritten: [], warnings: [], manualActions: [], errors: [] }
		return {
			success: true,
			filesWritten: providerResult.filesWritten,
			filesSkipped: [],
			backupDir: null,
			warnings: providerResult.warnings,
			manualActions: providerResult.manualActions,
			errors: providerResult.errors,
			historyDuplicatesSkipped: 0,
		}
	}

	const { universalConvert, universalWrite } = await import("@devo/configconv")

	// biome-ignore lint/suspicious/noExplicitAny: scanResult is dynamically typed from IPC
	const conversion = universalConvert(scanResult as any, { to: "devo" })

	const writeResult = await universalWrite(conversion, {
		backup: true,
		mergeStrategy: "preserve-existing",
	})

	const allWarnings = [...conversion.report.warnings]
	const allManualActions = [...conversion.report.manualActions]
	const allErrors = [...conversion.report.errors]
	const allFilesWritten = [...writeResult.filesWritten]
	let historyDuplicatesSkipped = 0

	if (provider === "claude-code" && categories.includes("config")) {
		const providerResult = await executeClaudeCodeProviderMigration(scanResult, requestNative)
		allFilesWritten.push(...providerResult.filesWritten)
		allWarnings.push(...providerResult.warnings)
		allManualActions.push(...providerResult.manualActions)
		allErrors.push(...providerResult.errors)
	}

	// Write history sessions if selected
	if (categories.includes("history")) {
		try {
			const historyResult = await executeHistoryMigration(provider, scanResult)
			allFilesWritten.push(...historyResult.filesWritten)
			historyDuplicatesSkipped = historyResult.duplicatesSkipped

			if (historyResult.duplicatesSkipped > 0) {
				allWarnings.push(
					`${historyResult.duplicatesSkipped} chat session${historyResult.duplicatesSkipped === 1 ? " was" : "s were"} already imported and skipped.`,
				)
			}
		} catch (err) {
			allErrors.push(
				`History migration failed: ${err instanceof Error ? err.message : String(err)}`,
			)
		}
	}

	return {
		success: true,
		filesWritten: allFilesWritten,
		filesSkipped: writeResult.filesSkipped,
		backupDir: writeResult.backupDir ?? null,
		warnings: allWarnings,
		manualActions: allManualActions,
		errors: allErrors,
		historyDuplicatesSkipped,
	}
}

/**
 * Execute history migration: convert sessions and write to Devo storage.
 * Uses the detailed writer with deduplication and sends progress to the renderer.
 */
async function executeHistoryMigration(
	provider: MigrationProvider,
	scanResult: unknown,
): Promise<{ filesWritten: string[]; duplicatesSkipped: number }> {
	// biome-ignore lint/suspicious/noExplicitAny: dynamically typed from IPC
	const result = scanResult as any

	const sendProgress = (phase: string, current: number, total: number, skipped: number) => {
		for (const win of BrowserWindow.getAllWindows()) {
			win.webContents.send("onboarding:migration-progress", {
				phase,
				current,
				total,
				duplicatesSkipped: skipped,
			})
		}
	}

	if (provider === "cursor" && result?.data?.history) {
		const { convertCursorHistory } = await import("@devo/configconv/converter/cursor-history")
		const { writeHistorySessionsDetailed } = await import("@devo/configconv/writer/history")

		sendProgress("converting", 0, 0, 0)
		const { sessions } = convertCursorHistory(result.data.history)

		if (sessions.length > 0) {
			const writeResult = await writeHistorySessionsDetailed(sessions, {
				onProgress: (progress) => {
					sendProgress(
						progress.phase,
						progress.sessionIndex,
						progress.sessionCount,
						progress.duplicatesSkipped,
					)
				},
			})
			return {
				filesWritten: writeResult.filesWritten,
				duplicatesSkipped: writeResult.duplicatesSkipped.length,
			}
		}
	} else if (provider === "claude-code" && result?.data?.history) {
		const { convertHistory } = await import("@devo/configconv/converter/history")
		const { writeHistorySessionsDetailed } = await import("@devo/configconv/writer/history")

		sendProgress("converting", 0, 0, 0)
		const { sessions } = await convertHistory(result.data.history)

		if (sessions.length > 0) {
			const writeResult = await writeHistorySessionsDetailed(sessions, {
				onProgress: (progress) => {
					sendProgress(
						progress.phase,
						progress.sessionIndex,
						progress.sessionCount,
						progress.duplicatesSkipped,
					)
				},
			})
			return {
				filesWritten: writeResult.filesWritten,
				duplicatesSkipped: writeResult.duplicatesSkipped.length,
			}
		}
	}

	return { filesWritten: [], duplicatesSkipped: 0 }
}

/**
 * Restores a migration backup.
 */
export async function restoreMigrationBackup(): Promise<{
	success: boolean
	restored: string[]
	removed: string[]
	errors: string[]
}> {
	const { restore } = await import("@devo/configconv")
	const result = await restore()
	return {
		success: result.errors.length === 0,
		restored: result.restored,
		removed: result.removed,
		errors: result.errors.map((e) => `${e.path}: ${e.error}`),
	}
}

// ============================================================
// Helpers
// ============================================================

/**
 * Build a ProviderDetection from a configconv scan result.
 * This is the "full scan" counterpart to the lightweight detectXxx() functions above.
 */
function buildDetectionFromScan(
	provider: MigrationProvider,
	// biome-ignore lint/suspicious/noExplicitAny: scan result is dynamically typed from import
	scanResult: any,
): ProviderDetection {
	const data = scanResult.data

	switch (provider) {
		case "claude-code":
			return buildClaudeCodeDetection(data)
		case "cursor":
			return buildCursorDetection(data)
		case "devo":
			return buildDevoDetection(data)
		case "opencode":
			return buildOpenCodeDetection(data)
	}
}

// biome-ignore lint/suspicious/noExplicitAny: scan result is dynamically typed
function buildClaudeCodeDetection(data: any): ProviderDetection {
	const mcpServerCount = countClaudeCodeMcpServers(data)
	const agentCount = countClaudeCodeItems(data, "agents")
	const commandCount = countClaudeCodeItems(data, "commands")
	let ruleCount = 0
	if (data.global?.claudeMd) ruleCount++
	for (const p of data.projects) {
		if (p.claudeMd) ruleCount++
	}
	const hasHooks = !!(data.global.settings as Record<string, unknown> | undefined)?.hooks
	const skillCount =
		data.global.skills.length +
		data.projects.reduce((sum: number, p: { skills: unknown[] }) => sum + p.skills.length, 0)

	const summaryParts: string[] = []
	if (data.global.settings) summaryParts.push("global settings")
	if (data.projects.length > 0)
		summaryParts.push(`${data.projects.length} project${data.projects.length === 1 ? "" : "s"}`)
	if (mcpServerCount > 0)
		summaryParts.push(`${mcpServerCount} MCP server${mcpServerCount === 1 ? "" : "s"}`)
	if (agentCount > 0) summaryParts.push(`${agentCount} agent${agentCount === 1 ? "" : "s"}`)

	return {
		provider: "claude-code",
		found: true,
		label: PROVIDER_LABELS["claude-code"],
		summary: `Found ${summaryParts.join(", ")}`,
		hasGlobalSettings: !!data.global.settings || !!data.global.userState,
		hasPermissions: !!data.global.settings,
		projectCount: data.projects.length,
		mcpServerCount,
		agentCount,
		commandCount,
		ruleCount,
		hasHooks,
		skillCount,
		totalSessions: data.history?.totalSessions ?? 0,
		totalMessages: data.history?.totalMessages ?? 0,
	}
}

// biome-ignore lint/suspicious/noExplicitAny: scan result is dynamically typed
function buildCursorDetection(data: any): ProviderDetection {
	const mcpServerCount = data.global.mcpJson?.mcpServers
		? Object.keys(data.global.mcpJson.mcpServers).length
		: 0
	const agentCount = data.global.agents?.length ?? 0
	const commandCount = data.global.commands?.length ?? 0
	const skillCount = data.global.skills?.length ?? 0
	const hasPermissions = !!data.global.cliConfig

	let ruleCount = 0
	for (const p of data.projects ?? []) {
		ruleCount += p.rules?.length ?? 0
		if (p.cursorRules) ruleCount++
	}

	const totalSessions = data.history?.totalSessions ?? 0
	const totalMessages = data.history?.totalMessages ?? 0

	const summaryParts: string[] = []
	if (mcpServerCount > 0)
		summaryParts.push(`${mcpServerCount} MCP server${mcpServerCount === 1 ? "" : "s"}`)
	if (hasPermissions) summaryParts.push("permissions")
	if (agentCount > 0) summaryParts.push(`${agentCount} agent${agentCount === 1 ? "" : "s"}`)
	if (commandCount > 0) summaryParts.push(`${commandCount} command${commandCount === 1 ? "" : "s"}`)
	if (skillCount > 0) summaryParts.push(`${skillCount} skill${skillCount === 1 ? "" : "s"}`)
	if (totalSessions > 0)
		summaryParts.push(`${totalSessions} chat session${totalSessions === 1 ? "" : "s"}`)

	return {
		provider: "cursor",
		found: true,
		label: PROVIDER_LABELS.cursor,
		summary: `Found ${summaryParts.join(", ") || "configuration"}`,
		hasGlobalSettings: !!data.global.mcpJson || hasPermissions,
		hasPermissions,
		projectCount: data.projects?.length ?? 0,
		mcpServerCount,
		agentCount,
		commandCount,
		ruleCount,
		hasHooks: false,
		skillCount,
		totalSessions,
		totalMessages,
	}
}

// biome-ignore lint/suspicious/noExplicitAny: scan result is dynamically typed
function buildDevoDetection(data: any): ProviderDetection {
	let mcpServerCount = 0
	if (data.global.config?.mcp) {
		mcpServerCount += Object.keys(data.global.config.mcp).length
	}
	const agentCount = data.global.agents?.length ?? 0
	const commandCount = data.global.commands?.length ?? 0
	const skillCount = data.global.skills?.length ?? 0
	const ruleCount = data.global.agentsMd ? 1 : 0
	const hasPermissions =
		!!data.global.config?.permission && Object.keys(data.global.config.permission).length > 0

	const summaryParts: string[] = []
	if (data.global.config) summaryParts.push("global config")
	if (mcpServerCount > 0)
		summaryParts.push(`${mcpServerCount} MCP server${mcpServerCount === 1 ? "" : "s"}`)
	if (ruleCount > 0) summaryParts.push("rules")
	if (agentCount > 0) summaryParts.push(`${agentCount} agent${agentCount === 1 ? "" : "s"}`)

	return {
		provider: "devo",
		found: true,
		label: PROVIDER_LABELS.devo,
		summary: `Found ${summaryParts.join(", ") || "configuration"}`,
		hasGlobalSettings: !!data.global.config,
		hasPermissions,
		projectCount: data.projects?.length ?? 0,
		mcpServerCount,
		agentCount,
		commandCount,
		ruleCount,
		hasHooks: false,
		skillCount,
		totalSessions: 0,
		totalMessages: 0,
	}
}

// biome-ignore lint/suspicious/noExplicitAny: scan result is dynamically typed
function buildOpenCodeDetection(data: any): ProviderDetection {
	const providerEntries = Object.entries(data.global?.config?.provider ?? {}) as Array<[
		string,
		{ models?: Record<string, unknown> },
	]>
	const providerCount = providerEntries.length
	const modelCount = providerEntries.reduce(
		(sum, [, provider]) => sum + Object.keys(provider.models ?? {}).length,
		0,
	)
	const hasConfig = !!data.global?.config || (data.global?.parseErrors?.length ?? 0) > 0
	const summaryParts: string[] = []

	if (providerCount > 0) {
		summaryParts.push(`${providerCount} provider${providerCount === 1 ? "" : "s"}`)
	}
	if (modelCount > 0) {
		summaryParts.push(`${modelCount} model${modelCount === 1 ? "" : "s"}`)
	}
	if (hasConfig && summaryParts.length === 0) summaryParts.push("configuration")

	return {
		provider: "opencode",
		found: hasConfig,
		label: PROVIDER_LABELS.opencode,
		summary: hasConfig ? `Found ${summaryParts.join(", ")}` : "No OpenCode configuration detected",
		hasGlobalSettings: hasConfig,
		hasPermissions: false,
		projectCount: 0,
		mcpServerCount: 0,
		agentCount: 0,
		commandCount: 0,
		ruleCount: 0,
		hasHooks: false,
		skillCount: 0,
		totalSessions: 0,
		totalMessages: 0,
	}
}

// biome-ignore lint/suspicious/noExplicitAny: scan result is dynamically imported
function countClaudeCodeMcpServers(data: any): number {
	let count = 0
	// Global MCP from user state
	if (data.global.userState?.projects) {
		for (const project of Object.values(data.global.userState.projects) as Array<
			Record<string, unknown>
		>) {
			if (project.mcpServers && typeof project.mcpServers === "object") {
				count += Object.keys(project.mcpServers).length
			}
		}
	}
	// Per-project MCP
	for (const project of data.projects) {
		if (project.mcpJson?.mcpServers) {
			count += Object.keys(project.mcpJson.mcpServers).length
		}
	}
	return count
}

// biome-ignore lint/suspicious/noExplicitAny: scan result is dynamically imported
function countClaudeCodeItems(data: any, field: string): number {
	let count = 0
	for (const project of data.projects) {
		count += project[field]?.length ?? 0
	}
	return count
}
