/**
 * Automation executor -- runs agent sessions via the Devo SDK.
 *
 * Given an automation config and workspace directory, the executor:
 * 1. Creates a worktree (if useWorktree is enabled)
 * 2. Creates an Devo session with the appropriate permission ruleset
 * 3. Sends the automation prompt (with memory file context)
 * 4. Monitors Native events until session goes idle or times out
 * 5. Captures results (summary, branch, diffs) and updates the run record
 * 6. Auto-archives if the agent reports nothing actionable
 *
 * Modeled after Devo's `run` CLI (packages/devo/src/cli/cmd/run.ts).
 */

import fs from "node:fs"
import path from "node:path"
import type { DevoClient } from "@devo-ai/sdk/v2/client"
import { createLogger } from "../logger"
import { createAutomationClient } from "./devo-client"
import { getConfigDir } from "./paths"
import { buildPermissionRuleset } from "./permission-policy"
import type { AutomationConfig, PermissionPreset } from "./types"

const log = createLogger("automation-executor")

/** Default timeout (in ms) for individual SDK calls (session.create, promptAsync, etc.). */
const SDK_CALL_TIMEOUT_MS = 60_000

/**
 * Wraps a promise with a timeout. Rejects with a descriptive error if the
 * promise doesn't settle within `ms` milliseconds.
 */
function withTimeout<T>(promise: Promise<T>, ms: number, label: string): Promise<T> {
	return new Promise<T>((resolve, reject) => {
		const timer = setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms)
		promise.then(
			(v) => {
				clearTimeout(timer)
				resolve(v)
			},
			(err) => {
				clearTimeout(timer)
				reject(err)
			},
		)
	})
}

// ============================================================
// Permission presets
// ============================================================

// ============================================================
// Memory file
// ============================================================

/**
 * Returns the path to the automation's memory file.
 * Lives at ~/.config/devo/automations/<id>/memory.md
 */
function getMemoryFilePath(automationId: string): string {
	return path.join(getConfigDir(), "automations", automationId, "memory.md")
}

/**
 * Reads the memory file content, or returns empty string if it doesn't exist.
 */
function readMemoryFile(automationId: string): string {
	const memPath = getMemoryFilePath(automationId)
	try {
		return fs.readFileSync(memPath, "utf-8")
	} catch {
		return ""
	}
}

/**
 * Builds the system prompt addendum that tells the agent about the memory file.
 */
function buildSystemPrompt(automationId: string, automationName: string): string {
	const memPath = getMemoryFilePath(automationId)
	const memory = readMemoryFile(automationId)

	const lines = [
		`You are running as an automated agent for the "${automationName}" automation.`,
		"",
		"IMPORTANT RULES:",
		"- Do NOT ask questions or enter plan mode. You must complete the task autonomously.",
		"- At the END of your response, include a line: `Actionable: yes` or `Actionable: no`",
		"  to indicate whether your findings require human review.",
		"",
		`You have a persistent memory file at: ${memPath}`,
		"You can read it and write to it to remember context across runs.",
	]

	if (memory) {
		lines.push("", "Current memory file contents:", "```", memory, "```")
	}

	return lines.join("\n")
}

// ============================================================
// Event monitoring
// ============================================================

export interface ExecutionResult {
	sessionId: string
	worktreePath: string | null
	title: string
	summary: string
	hasActionable: boolean
	branch: string | null
	error: string | null
}

/**
 * Monitors Native events for a session until it goes idle, errors, or times out.
 *
 * Returns collected text output and error information.
 */
async function monitorSession(
	client: DevoClient,
	sessionId: string,
	timeoutMs: number,
	signal: AbortSignal,
	permissionPreset: PermissionPreset,
): Promise<{ text: string; error: string | null }> {
	const textParts: string[] = []
	let error: string | null = null

	const timeoutPromise = new Promise<"timeout">((resolve) => {
		const timer = setTimeout(() => resolve("timeout"), timeoutMs)
		signal.addEventListener("abort", () => clearTimeout(timer), { once: true })
	})

	const eventPromise = (async (): Promise<"done"> => {
		try {
			log.debug("Subscribing to Native events", { sessionId })
			const result = await client.event.subscribe()
			log.debug("Native event stream connected", { sessionId })
			for await (const event of result.stream) {
				if (signal.aborted) break

				// biome-ignore lint/suspicious/noExplicitAny: Native events have dynamic types not fully covered by SDK
				const evt = event as any

				// Capture text output from the assistant
				if (evt.type === "message.part.updated") {
					const part = evt.properties?.part
					if (
						part?.sessionID === sessionId &&
						part?.type === "text" &&
						part?.time?.end &&
						part?.text
					) {
						textParts.push(part.text)
					}
				}

				// Capture errors
				if (evt.type === "session.error") {
					if (evt.properties?.sessionID === sessionId && evt.properties?.error) {
						const errObj = evt.properties.error
						const errMsg = errObj.data?.message ?? errObj.name ?? "Unknown error"
						error = error ? `${error}\n${errMsg}` : String(errMsg)
						log.error("Session error during automation", {
							sessionId,
							error: errMsg,
						})
					}
				}

				// A configured restrictive policy may reject a request. The default
				// policy remains waiting for an external controller instead of silently
				// changing the user's effective permissions.
				if (evt.type === "permission.asked" && permissionPreset === "read-only") {
					if (evt.properties?.sessionID === sessionId) {
						log.warn("Auto-rejecting permission request during automation", {
							sessionId,
							permission: evt.properties.permission,
						})
						try {
							await client.permission.reply({
								requestID: evt.properties.id,
								reply: "reject",
							})
						} catch (rejectErr) {
							log.warn("Failed to reject permission request", rejectErr)
						}
					}
				}

				// Session went idle -- we're done
				if (
					evt.type === "session.status" &&
					evt.properties?.sessionID === sessionId &&
					evt.properties?.status?.type === "idle"
				) {
					break
				}
			}
		} catch (err) {
			if (!signal.aborted) {
				log.error("Native event monitoring error", err)
				error = error
					? `${error}\nNative event error: ${err instanceof Error ? err.message : String(err)}`
					: `Native event error: ${err instanceof Error ? err.message : String(err)}`
			}
		}
		return "done"
	})()

	const outcome = await Promise.race([eventPromise, timeoutPromise])

	if (outcome === "timeout") {
		log.warn("Session monitoring timed out", { sessionId, timeoutMs })
		try {
			await client.session.abort({ sessionID: sessionId })
			log.info("Session aborted after timeout", { sessionId })
		} catch {
			log.warn("Failed to abort session after timeout", { sessionId })
		}
		error = error
			? `${error}\nSession timed out after ${Math.round(timeoutMs / 1000)}s`
			: `Session timed out after ${Math.round(timeoutMs / 1000)}s`
	}

	return { text: textParts.join("\n"), error }
}

/**
 * Parses the "Actionable: yes/no" line from the agent's output.
 * Defaults to true (require review) if not found.
 */
function parseActionable(text: string): boolean {
	const match = text.match(/Actionable:\s*(yes|no)/i)
	if (!match) return true
	return match[1].toLowerCase() === "yes"
}

// ============================================================
// Model resolution
// ============================================================

/**
 * Parses a model string in "providerID/modelID" format into the object
 * shape expected by the Devo SDK. Returns undefined if the string
 * is empty or malformed.
 */
function parseModelRef(modelStr: string): { providerID: string; modelID: string } | undefined {
	if (!modelStr) return undefined
	const slashIndex = modelStr.indexOf("/")
	if (slashIndex <= 0 || slashIndex === modelStr.length - 1) return undefined
	return {
		providerID: modelStr.slice(0, slashIndex),
		modelID: modelStr.slice(slashIndex + 1),
	}
}

// ============================================================
// Main executor
// ============================================================

/**
 * Callback fired as soon as the Devo session is created, before the
 * prompt is sent and monitoring begins. This allows the caller to persist
 * the sessionId immediately so the renderer can show the live session.
 */
export type OnSessionCreated = (info: {
	sessionId: string
	worktreePath: string | null
}) => void | Promise<void>

/**
 * Executes a single automation run against a workspace.
 *
 * @param config      The automation config (from disk)
 * @param workspace   The project directory to run against
 * @param onSessionCreated  Optional callback invoked as soon as the session
 *                          is created, before monitoring begins
 * @returns Execution result with session info, summary, and actionability
 */
export async function executeRun(
	config: AutomationConfig & { id: string; prompt: string },
	workspace: string,
	onSessionCreated?: OnSessionCreated,
): Promise<ExecutionResult> {
	const client = createAutomationClient(workspace)
	if (!client) {
		log.error("Cannot execute run: no Devo server running", {
			automationId: config.id,
			workspace,
		})
		return {
			sessionId: "",
			worktreePath: null,
			title: config.name,
			summary: "",
			hasActionable: false,
			branch: null,
			error: "No Devo server running",
		}
	}

	const abortController = new AbortController()
	let worktreePath: string | null = null
	let sessionId = ""
	const runStartTime = Date.now()
	log.info("Starting execution", {
		automationId: config.id,
		automationName: config.name,
		workspace,
		useWorktree: config.execution.useWorktree,
		timeoutSec: config.execution.timeout,
		model: config.execution.model || "default",
		agent: config.execution.agent || "default",
		variant: config.execution.variant || "default",
	})

	try {
		// --- Step 1: Create worktree (if enabled) ---
		if (config.execution.useWorktree) {
			log.info("Creating worktree", { automationId: config.id, workspace })
			const wtStart = Date.now()
			try {
				const result = await withTimeout(
					client.worktree.create({
						worktreeCreateInput: {
							name: `automation-${config.id}-${Date.now()}`,
						},
					}),
					SDK_CALL_TIMEOUT_MS,
					"worktree.create",
				)
				log.info("Worktree created", {
					automationId: config.id,
					durationMs: Date.now() - wtStart,
				})
				// biome-ignore lint/suspicious/noExplicitAny: worktree API response shape not fully typed
					const data = (result as any).data
				if (data?.directory) {
					worktreePath = data.directory
					log.info("Worktree created", {
						directory: worktreePath,
						branch: data.branch,
					})
				}
			} catch (err) {
				log.warn("Worktree creation failed, falling back to main workspace", {
					automationId: config.id,
					durationMs: Date.now() - wtStart,
					error: err instanceof Error ? err.message : String(err),
				})
				// Continue without worktree -- run in the main workspace
			}
		}

		// --- Step 2: Create session with permission ruleset ---
		const permissionRuleset = buildPermissionRuleset(config.execution.permissionPreset ?? "default")

		// If running in a worktree, create a client scoped to that directory
		const sessionClient = worktreePath ? (createAutomationClient(worktreePath) ?? client) : client

		log.info("Creating session", {
			automationId: config.id,
			permissionPreset: config.execution.permissionPreset ?? "default",
			worktreePath,
		})
		const sessionStart = Date.now()
		const sessionResult = await withTimeout(
			sessionClient.session.create({
				title: `[Auto] ${config.name}`,
				permission: permissionRuleset,
			}),
			SDK_CALL_TIMEOUT_MS,
			"session.create",
		)
		log.info("Session created", {
			automationId: config.id,
			durationMs: Date.now() - sessionStart,
		})

		// biome-ignore lint/suspicious/noExplicitAny: session create response varies across SDK versions
			const session = (sessionResult as any).data
		if (!session?.id) {
			return {
				sessionId: "",
				worktreePath,
				title: config.name,
				summary: "",
				hasActionable: false,
				branch: null,
				error: "Failed to create session: no session ID returned",
			}
		}

		sessionId = session.id
		log.info("Session created for automation", {
			sessionId,
			automationId: config.id,
			worktreePath,
		})

		// Notify caller immediately so sessionId can be persisted and the
		// renderer can start showing the live session view
		if (onSessionCreated) {
			try {
				await onSessionCreated({ sessionId, worktreePath })
			} catch (cbErr) {
				log.warn("onSessionCreated callback failed", cbErr)
			}
		}

		// --- Step 3: Send prompt ---
		const systemPrompt = buildSystemPrompt(config.id, config.name)

		// Parse model string (format: "providerID/modelID") if configured
		const model = config.execution.model ? parseModelRef(config.execution.model) : undefined
		const agent = config.execution.agent || undefined
		const variant = config.execution.variant || undefined

		log.info("Sending prompt", {
			automationId: config.id,
			sessionId,
			model: config.execution.model || "default",
			agent: agent || "default",
			variant: variant || "default",
			promptLength: config.prompt.length,
		})
		const promptStart = Date.now()
		await withTimeout(
			sessionClient.session.promptAsync({
				sessionID: sessionId,
				system: systemPrompt,
				parts: [{ type: "text", text: config.prompt }],
				model,
				agent,
				variant,
			}),
			SDK_CALL_TIMEOUT_MS,
			"session.promptAsync",
		)
		log.info("Prompt sent, starting monitor", {
			automationId: config.id,
			sessionId,
			sendDurationMs: Date.now() - promptStart,
			monitorTimeoutSec: config.execution.timeout,
		})

		// --- Step 4: Monitor until idle or timeout ---
		const monitorStart = Date.now()
		const { text, error } = await monitorSession(
			sessionClient,
			sessionId,
			config.execution.timeout * 1000,
			abortController.signal,
			config.execution.permissionPreset ?? "default",
		)
		log.info("Monitor completed", {
			automationId: config.id,
			sessionId,
			monitorDurationMs: Date.now() - monitorStart,
			outputLength: text.length,
			hadError: !!error,
		})

		// --- Step 5: Capture results ---
		const hasActionable = error ? true : parseActionable(text)

		// Try to get session summary/diff info
		let branch: string | null = null
		try {
			const sessionInfo = await sessionClient.session.get({ sessionID: sessionId })
			// biome-ignore lint/suspicious/noExplicitAny: session response shape varies
			const info = sessionInfo.data as any
			if (info?.summary?.diffs?.length > 0) {
				// The branch is available from the worktree, not the session directly
				branch = worktreePath ? `automation/${config.id}` : null
			}
		} catch {
			// Non-critical, continue without summary
		}

		// Build a concise summary from the text output
		const summary = text
			? text.length > 2000
				? `${text.slice(0, 2000)}...`
				: text
			: error
				? `Error: ${error}`
				: "Automation completed with no output"

		const totalMs = Date.now() - runStartTime
		log.info("Execution finished", {
			automationId: config.id,
			sessionId,
			totalDurationMs: totalMs,
			hasActionable,
			hasError: !!error,
			branch,
			summaryLength: summary.length,
		})

		return {
			sessionId,
			worktreePath,
			title: config.name,
			summary,
			hasActionable,
			branch,
			error,
		}
	} catch (err) {
		const totalMs = Date.now() - runStartTime
		log.error("Automation execution failed", {
			automationId: config.id,
			workspace,
			sessionId: sessionId || "none",
			totalDurationMs: totalMs,
			error: err instanceof Error ? err.message : String(err),
		})
		return {
			sessionId,
			worktreePath,
			title: config.name,
			summary: "",
			hasActionable: false,
			branch: null,
			error: err instanceof Error ? err.message : "Unknown execution error",
		}
	} finally {
		abortController.abort()
	}
}
