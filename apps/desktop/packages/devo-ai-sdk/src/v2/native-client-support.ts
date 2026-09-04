// @ts-nocheck

import type { DevoNativeTransport } from "./client"
export type SessionConfigOption = {
	id: string
	name?: string
	type: string
	currentValue?: unknown
	options?: unknown
	[key: string]: unknown
}

export class AsyncEventQueue<T> implements AsyncIterable<T> {
	private values: T[] = []
	private waiters: Array<(value: IteratorResult<T>) => void> = []
	private closed = false

	push(value: T): void {
		const waiter = this.waiters.shift()
		if (waiter) {
			waiter({ value, done: false })
			return
		}
		this.values.push(value)
	}

	close(): void {
		this.closed = true
		for (const waiter of this.waiters.splice(0)) {
			waiter({ value: undefined as T, done: true })
		}
	}

	[Symbol.asyncIterator](): AsyncIterator<T> {
		return {
			next: () => {
				const value = this.values.shift()
				if (value) return Promise.resolve({ value, done: false })
				if (this.closed) return Promise.resolve({ value: undefined as T, done: true })
				return new Promise<IteratorResult<T>>((resolve) => this.waiters.push(resolve))
			},
		}
	}
}

export function stableId(value: string): string {
	let hash = 5381
	for (let index = 0; index < value.length; index++) {
		hash = (hash * 33) ^ value.charCodeAt(index)
	}
	return Math.abs(hash >>> 0).toString(16)
}

export function defaultCwd(): string {
	if (typeof location !== "undefined") return "/"
	return process.env.HOME ?? process.cwd()
}

let sharedIpcTransport: DevoNativeTransport | null = null

/** Must match `DEVO_NATIVE_IPC_ERROR` in apps/desktop/src/shared/native-ipc-error.ts */
const DEVO_NATIVE_IPC_ERROR = "$devoNativeError"

function throwIfNativeIpcError(result: unknown): void {
	if (!result || typeof result !== "object") return
	const envelope = (result as Record<string, unknown>)[DEVO_NATIVE_IPC_ERROR]
	if (!envelope || typeof envelope !== "object") return
	const record = envelope as Record<string, unknown>
	const error = new Error(
		typeof record.message === "string" ? record.message : "Devo Native request failed",
	) as Error & { code?: string }
	if (typeof record.code === "string") error.code = record.code
	throw error
}

export function createIpcTransport(): DevoNativeTransport {
	if (sharedIpcTransport) return sharedIpcTransport

	const api = globalThis.window?.devo?.native
	if (!api) throw new Error("window.devo.native is not available")
	sharedIpcTransport = {
		request: async (method, params, directory) => {
			const result = await api.request({ method, params, directory })
			throwIfNativeIpcError(result)
			return result
		},
		notify: (method, params, directory) => api.notify({ method, params, directory }),
		respond: (id, result) => api.respond({ id, result }),
		subscribe: (listener) => api.subscribe(listener),
		connected: () => api.connected(),
	}
	return sharedIpcTransport
}

export function providerDataFromConfigOptions(configOptions: SessionConfigOption[]): {
	default: Record<string, string>
	providers: any[]
} {
	const modelOption = configOptions.find((option) => option.id === "model")
	if (!modelOption) return { default: {}, providers: [] }
	const currentValue = typeof modelOption.currentValue === "string" ? modelOption.currentValue : undefined
	const reasoningOption = configOptions.find((option) => option.id === "thought_level")
	const fallbackReasoningVariants = variantsFromConfigOption(reasoningOption)
	const currentVariant =
		typeof reasoningOption?.currentValue === "string" ? reasoningOption.currentValue : undefined
	const models = Object.fromEntries(
		flattenSelectOptions(modelOption.options).map((option) => {
			const perModelVariants = variantsFromAvailableEfforts(option.availableEfforts)
			const reasoningVariants =
				Object.keys(perModelVariants).length > 0 ? perModelVariants : fallbackReasoningVariants
			const hasReasoningVariants = Object.keys(reasoningVariants).length > 0
			const model = {
				name: option.name,
				description: option.description,
				capabilities: {
					reasoning: hasReasoningVariants,
					input: { image: false, pdf: false },
					attachment: false,
				},
			}
			if (!hasReasoningVariants) return [option.value, model]
			const isCurrentModel = option.value === currentValue
			const variantOnCurrent =
				isCurrentModel && currentVariant && currentVariant in reasoningVariants
					? currentVariant
					: undefined
			return [
				option.value,
				{
					...model,
					variants: reasoningVariants,
					...(variantOnCurrent !== undefined ? { currentVariant: variantOnCurrent } : {}),
					allowDefaultVariant: false,
				},
			]
		}),
	)
	if (Object.keys(models).length === 0) return { default: {}, providers: [] }
	return {
		default: currentValue ? { session: currentValue } : {},
		providers: [{ id: "session", name: "Session", models }],
	}
}

export function configDataFromConfigOptions(configOptions: SessionConfigOption[]): any {
	const modelOption = configOptions.find((option) => option.id === "model")
	const currentValue = typeof modelOption?.currentValue === "string" ? modelOption.currentValue : undefined
	return currentValue ? { model: `session/${currentValue}` } : {}
}

export function questionInfoFromNative(question: unknown): any {
	const value = question && typeof question === "object" ? (question as Record<string, unknown>) : {}
	const options = Array.isArray(value.options)
		? value.options.map((option) => {
				const optionValue =
					option && typeof option === "object" ? (option as Record<string, unknown>) : {}
				return {
					label: String(optionValue.label ?? ""),
					description: String(optionValue.description ?? ""),
				}
			})
		: []
	return {
		id: String(value.id ?? ""),
		header: String(value.header ?? ""),
		question: String(value.question ?? ""),
		options,
		isOther: Boolean(value.isOther ?? value.is_other ?? true),
		isSecret: Boolean(value.isSecret ?? value.is_secret ?? false),
	}
}

export function partTime(
	existingPart: any,
	now: number,
	options?: { start?: number; end?: number },
): { start: number; end?: number } {
	const start =
		typeof options?.start === "number" && Number.isFinite(options.start)
			? options.start
			: typeof existingPart?.time?.start === "number"
				? existingPart.time.start
				: now
	const end =
		typeof options?.end === "number" && Number.isFinite(options.end)
			? options.end
			: typeof existingPart?.time?.end === "number"
				? existingPart.time.end
				: undefined
	return end === undefined ? { start } : { start, end }
}

export function toolCallIdFromUpdate(update: Record<string, unknown>, now: number): string {
	return String(update.toolCallId ?? update.callID ?? update.id ?? `tool-${now}`)
}

export function toolPartFromUpdate(
	sessionId: string,
	update: Record<string, unknown>,
	existingPart: any,
	now: number,
): any {
	const toolCallId = toolCallIdFromUpdate(update, now)
	const messageID = `tool-${toolCallId}`
	const legacyContent = Array.isArray(update.content) ? update.content : undefined
	const legacyLocations = Array.isArray(update.locations) ? update.locations : undefined
	const incomingInput = objectFromValue(update.rawInput ?? update.input)
	const existingInput = objectFromValue(existingPart?.state?.input)
	// Prefer the update's input when present, but keep richer fields from a prior
	// toolCall start (oldString/newString/content) that fileChange completions omit.
	const mergedBase =
		Object.keys(incomingInput).length > 0
			? mergeToolInputs(existingInput, incomingInput)
			: existingInput
	const input = enrichedToolInput(mergedBase, legacyContent)
	const tool = resolvedToolName(update, existingPart, input, toolCallId)
	const title = resolvedToolTitle(update, existingPart, tool, toolCallId)
	const metadata = {
		...objectFromValue(existingPart?.state?.metadata),
		...(legacyContent ? { legacyContent } : {}),
		...(legacyLocations ? { legacyLocations } : {}),
	}
	const time = existingPart?.state?.time ?? { start: now }
	const status = toolStateStatus(update.status, existingPart?.state?.status)
	const baseState = { input, title, metadata }
	const state =
		status === "pending"
			? { status, ...baseState, raw: rawString(update.rawInput), time }
			: status === "running"
				? { status, ...baseState, time }
				: status === "error"
					? { status, ...baseState, error: toolOutputString(update), time: { ...time, end: time.end ?? now } }
					: {
							status: "completed",
							...baseState,
							output: toolOutputString(update),
							time: { ...time, end: time.end ?? now },
						}
	return {
		id: `${messageID}-part`,
		sessionID: sessionId,
		messageID,
		type: "tool",
		callID: toolCallId,
		tool,
		state,
	}
}

/** Merge fileChange completion fields onto an existing toolCall start without wiping diffs. */
function mergeToolInputs(
	existing: Record<string, unknown>,
	incoming: Record<string, unknown>,
): Record<string, unknown> {
	const merged = { ...existing, ...incoming }
	for (const key of ["oldString", "newString", "content"] as const) {
		if (typeof existing[key] === "string" && incoming[key] == null) {
			merged[key] = existing[key]
		}
	}
	return merged
}

export function statusFromDevo(status?: string): any {
	const normalized = status
		?.replace(/([a-z])([A-Z])/g, "$1_$2")
		.replace(/-/g, "_")
		.toLowerCase()
	switch (normalized) {
		case "active":
		case "active_turn":
		case "running":
		case "busy":
		case "waiting_client":
			return { type: "busy" }
		case "failed":
		case "error":
			return { type: "error" }
		case "idle":
		case "archived":
		case "unloaded":
		case undefined:
			return { type: "idle" }
		default:
			return { type: "idle" }
	}
}

export function sessionErrorEvent(sessionID: string, error: unknown): any {
	return {
		type: "session.error",
		properties: {
			sessionID,
			error: {
				name: "Error",
				data: { message: error instanceof Error ? error.message : String(error) },
			},
		},
	}
}

function objectFromValue(value: unknown): Record<string, unknown> {
	return value && typeof value === "object" && !Array.isArray(value)
		? (value as Record<string, unknown>)
		: {}
}

function stringFromValue(value: unknown): string | undefined {
	return typeof value === "string" && value.trim() ? value : undefined
}

function resolvedToolName(
	update: Record<string, unknown>,
	existingPart: any,
	input: Record<string, unknown>,
	toolCallId: string,
): string {
	const explicitTool = stringFromValue(update.tool)
	if (explicitTool) return explicitTool

	const kind = stringFromValue(update.kind)
	if (kind) return toolNameFromUpdateKind(kind, input)

	const existingTool = stringFromValue(existingPart?.tool)
	if (existingTool && existingTool !== toolCallId && existingTool !== existingPart?.callID) {
		return existingTool
	}

	const title = stringFromValue(update.title) ?? stringFromValue(existingPart?.state?.title)
	return inferToolNameFromInput(input, title)
}

function resolvedToolTitle(
	update: Record<string, unknown>,
	existingPart: any,
	tool: string,
	toolCallId: string,
): string {
	const title = stringFromValue(update.title)
	if (title) return title

	const existingTitle = stringFromValue(existingPart?.state?.title)
	if (existingTitle && existingTitle !== toolCallId && existingTitle !== existingPart?.callID) {
		return existingTitle
	}

	return tool
}

function toolNameFromUpdateKind(kind: string, input: Record<string, unknown>): string {
	switch (kind) {
		case "read":
			return "read"
		case "write":
			return "write"
		case "edit":
		case "delete":
		case "move":
		case "apply_patch":
			return kind === "apply_patch" ? "apply_patch" : "edit"
		case "search":
			return "grep"
		case "execute":
			return "bash"
		case "fetch":
			return "webfetch"
		case "think":
			return "think"
		case "question":
		case "request_user_input":
			return "request_user_input"
		case "other":
			return inferToolNameFromInput(input)
		default:
			return inferToolNameFromInput(input, kind)
	}
}

function inferToolNameFromInput(input: Record<string, unknown>, title?: string): string {
	if (Array.isArray(input.questions)) return "request_user_input"
	if (typeof input.command === "string") return "bash"
	if (typeof input.url === "string") return "webfetch"
	if (typeof input.pattern === "string") return "grep"
	const changeType = typeof input.changeType === "string" ? input.changeType : undefined
	if (changeType === "add") return "write"
	if (changeType === "update" || changeType === "delete") return "edit"
	if (typeof input.unifiedDiff === "string" || typeof input.unified_diff === "string") return "edit"
	if (typeof input.filePath === "string" || typeof input.path === "string") {
		if (typeof input.oldString === "string" || typeof input.newString === "string") return "edit"
		if (typeof input.content === "string") return "write"
		return "read"
	}
	const normalizedTitle = title?.trim().toLowerCase()
	if (normalizedTitle?.startsWith("read")) return "read"
	if (normalizedTitle?.startsWith("edit") || normalizedTitle?.startsWith("patch")) return "edit"
	if (normalizedTitle?.startsWith("write")) return "write"
	if (normalizedTitle?.startsWith("search")) return "grep"
	if (normalizedTitle?.startsWith("fetch")) return "webfetch"
	if (normalizedTitle?.startsWith("run") || normalizedTitle?.startsWith("execute")) return "bash"
	if (normalizedTitle === "question" || normalizedTitle === "request_user_input") {
		return "request_user_input"
	}
	return "tool"
}

function rawString(value: unknown): string {
	if (typeof value === "string") return value
	if (value === undefined || value === null) return ""
	const record =
		value && typeof value === "object" && !Array.isArray(value)
			? (value as Record<string, unknown>)
			: undefined
	// Mixed tool results carry the human-readable body under `output` / `text`.
	// Do not JSON.stringify those — that turns real newlines into literal `\n`.
	if (record && !Array.isArray(record.files)) {
		if (typeof record.output === "string") return record.output
		if (typeof record.text === "string") return record.text
	}
	return JSON.stringify(value)
}

function toolOutputString(update: Record<string, unknown>): string {
	if ("rawOutput" in update) return rawString(update.rawOutput)
	const contentOutput = outputFromToolContent(update.content)
	if (contentOutput) return contentOutput
	return textFromUpdate(update)
}

function enrichedToolInput(
	baseInput: Record<string, unknown>,
	content: unknown[] | undefined,
): Record<string, unknown> {
	const input = { ...baseInput }
	const diff = content?.find(
		(item) => item && typeof item === "object" && (item as Record<string, unknown>).type === "diff",
	) as Record<string, unknown> | undefined
	if (!diff) return input
	if (typeof input.path !== "string" && typeof diff.path === "string") input.path = diff.path
	if (typeof input.filePath !== "string" && typeof diff.path === "string") input.filePath = diff.path
	if (typeof input.oldString !== "string") input.oldString = typeof diff.oldText === "string" ? diff.oldText : ""
	if (typeof input.newString !== "string") input.newString = typeof diff.newText === "string" ? diff.newText : ""
	return input
}

function outputFromToolContent(content: unknown): string {
	if (!Array.isArray(content)) return ""
	const textParts: string[] = []
	for (const item of content) {
		if (!item || typeof item !== "object") continue
		const value = item as Record<string, unknown>
		if (value.type === "content") {
			const text = textFromUpdate({ content: value.content })
			if (text) textParts.push(text)
		}
	}
	return textParts.join("\n\n")
}

function toolStateStatus(value: unknown, existingStatus: unknown): "completed" | "error" | "pending" | "running" {
	const status = typeof value === "string" ? value : existingStatus
	switch (status) {
		case "pending":
			return "pending"
		case "in_progress":
		case "inProgress":
		case "running":
			return "running"
		case "failed":
		case "cancelled":
		case "error":
			return "error"
		case "completed":
		default:
			return "completed"
	}
}

export function permissionOptionId(
	options: Array<{ optionId: string; kind: string }>,
	response: "once" | "always" | "reject",
): string {
	const preferred =
		response === "always"
			? options.find((option) => option.kind === "allow_always")
			: response === "once"
				? options.find((option) => option.kind === "allow_once")
				: options.find((option) => option.kind.startsWith("reject"))
	return preferred?.optionId ?? options[0]?.optionId ?? response
}

export function textFromUpdate(update: Record<string, unknown>): string {
	for (const key of ["text", "delta", "message"]) {
		const value = update[key]
		if (typeof value === "string") return value
	}
	const content = update.content
	if (typeof content === "string") return content
	if (content && typeof content === "object" && !Array.isArray(content) && "text" in content) {
		return String((content as { text: unknown }).text)
	}
	if (!Array.isArray(content)) return ""
	return content
		.map((item) => {
			if (item && typeof item === "object" && "text" in item) {
				return String((item as { text: unknown }).text)
			}
			if (!hasNestedTextContent(item)) return ""
			return String(((item as { content: { text: unknown } }).content).text)
		})
		.join("")
}

function flattenSelectOptions(options: unknown): Array<{
	value: string
	name: string
	description?: string
	availableEfforts?: unknown
}> {
	if (!Array.isArray(options)) return []
	const result: Array<{
		value: string
		name: string
		description?: string
		availableEfforts?: unknown
	}> = []
	for (const option of options) {
		if (!option || typeof option !== "object") continue
		const record = option as Record<string, unknown>
		const value = record.value
		const nestedOptions = record.options
		if (typeof value === "string") {
			result.push({
				value,
				name: String(record.name ?? value),
				description: typeof record.description === "string" ? String(record.description) : undefined,
				...(record.availableEfforts !== undefined
					? { availableEfforts: record.availableEfforts }
					: {}),
			})
			continue
		}
		result.push(...flattenSelectOptions(nestedOptions))
	}
	return result
}

function variantsFromAvailableEfforts(
	availableEfforts: unknown,
): Record<string, { name: string; description?: string }> {
	if (!Array.isArray(availableEfforts)) return {}
	return Object.fromEntries(
		flattenSelectOptions(availableEfforts).map((selectOption) => [
			selectOption.value,
			{
				name: selectOption.name,
				description: selectOption.description,
			},
		]),
	)
}

function variantsFromConfigOption(option?: SessionConfigOption): Record<string, { name: string; description?: string }> {
	if (!option) return {}
	return Object.fromEntries(
		flattenSelectOptions(option.options).map((selectOption) => [
			selectOption.value,
			{
				name: selectOption.name,
				description: selectOption.description,
			},
		]),
	)
}

function hasNestedTextContent(item: unknown): boolean {
	return (
		!!item &&
		typeof item === "object" &&
		"content" in item &&
		!!(item as { content: unknown }).content &&
		typeof (item as { content: unknown }).content === "object" &&
		"text" in ((item as { content: unknown }).content as Record<string, unknown>)
	)
}
