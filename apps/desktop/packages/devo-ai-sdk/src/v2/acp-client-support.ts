// @ts-nocheck

import type { DevoAcpTransport } from "./client"
import type { AcpSessionConfigOption } from "./generated"

export type AcpConfigOption = AcpSessionConfigOption

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

let sharedIpcTransport: DevoAcpTransport | null = null

export function createIpcTransport(): DevoAcpTransport {
	if (sharedIpcTransport) return sharedIpcTransport

	const api = globalThis.window?.devo?.acp
	if (!api) throw new Error("window.devo.acp is not available")
	sharedIpcTransport = {
		request: (method, params, directory) => api.request({ method, params, directory }),
		respond: (id, result) => api.respond({ id, result }),
		subscribe: (listener) => api.subscribe(listener),
		connected: () => api.connected(),
	}
	return sharedIpcTransport
}

export function providerDataFromConfigOptions(configOptions: AcpConfigOption[]): {
	default: Record<string, string>
	providers: any[]
} {
	const modelOption = configOptions.find((option) => option.id === "model")
	if (!modelOption) return { default: {}, providers: [] }
	const currentValue = typeof modelOption.currentValue === "string" ? modelOption.currentValue : undefined
	const reasoningOption = configOptions.find((option) => option.id === "thought_level")
	const reasoningVariants = variantsFromConfigOption(reasoningOption)
	const currentVariant =
		typeof reasoningOption?.currentValue === "string" ? reasoningOption.currentValue : undefined
	const hasReasoningVariants = Object.keys(reasoningVariants).length > 0
	const models = Object.fromEntries(
		flattenSelectOptions(modelOption.options).map((option) => {
			const model = {
				name: option.name,
				description: option.description,
				capabilities: {
					reasoning: hasReasoningVariants,
					input: { image: false, pdf: false },
					attachment: false,
				},
			}
			return [
				option.value,
				hasReasoningVariants
					? {
							...model,
							variants: reasoningVariants,
							currentVariant,
							allowDefaultVariant: false,
						}
					: model,
			]
		}),
	)
	if (Object.keys(models).length === 0) return { default: {}, providers: [] }
	return {
		default: currentValue ? { session: currentValue } : {},
		providers: [{ id: "session", name: "Session", models }],
	}
}

export function configDataFromConfigOptions(configOptions: AcpConfigOption[]): any {
	const modelOption = configOptions.find((option) => option.id === "model")
	const currentValue = typeof modelOption?.currentValue === "string" ? modelOption.currentValue : undefined
	return currentValue ? { model: `session/${currentValue}` } : {}
}

export function questionInfoFromAcp(question: unknown): any {
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

export function requestUserInputFromOriginalEvent(
	original: unknown,
): Record<string, unknown> | undefined {
	if (!original || typeof original !== "object") return undefined
	const event = original as Record<string, unknown>
	if (event.kind === "request_user_input") return event
	const legacy = event.RequestUserInput
	return legacy && typeof legacy === "object"
		? (legacy as Record<string, unknown>)
		: undefined
}

export function partTime(existingPart: any, now: number): { start: number; end?: number } {
	return existingPart?.time ?? { start: now }
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
	const acpContent = Array.isArray(update.content) ? update.content : undefined
	const acpLocations = Array.isArray(update.locations) ? update.locations : undefined
	const input = enrichedToolInput(
		objectFromValue(update.rawInput ?? update.input ?? existingPart?.state?.input),
		acpContent,
	)
	const tool = resolvedToolName(update, existingPart, input, toolCallId)
	const title = resolvedToolTitle(update, existingPart, tool, toolCallId)
	const metadata = {
		...objectFromValue(existingPart?.state?.metadata),
		...(acpContent ? { acpContent } : {}),
		...(acpLocations ? { acpLocations } : {}),
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

export function statusFromDevo(status?: string): any {
	const normalized = status
		?.replace(/([a-z])([A-Z])/g, "$1_$2")
		.replace(/-/g, "_")
		.toLowerCase()
	switch (normalized) {
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
	if (kind) return toolNameFromAcpKind(kind, input)

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

function toolNameFromAcpKind(kind: string, input: Record<string, unknown>): string {
	switch (kind) {
		case "read":
			return "read"
		case "edit":
		case "delete":
		case "move":
			return "edit"
		case "search":
			return "grep"
		case "execute":
			return "bash"
		case "fetch":
			return "webfetch"
		case "think":
			return "think"
		case "other":
			return inferToolNameFromInput(input)
		default:
			return inferToolNameFromInput(input)
	}
}

function inferToolNameFromInput(input: Record<string, unknown>, title?: string): string {
	if (typeof input.command === "string") return "bash"
	if (typeof input.url === "string") return "webfetch"
	if (typeof input.pattern === "string") return "grep"
	if (typeof input.filePath === "string" || typeof input.path === "string") {
		if (typeof input.oldString === "string" || typeof input.newString === "string") return "edit"
		if (typeof input.content === "string") return "write"
		return "read"
	}
	const normalizedTitle = title?.trim().toLowerCase()
	if (normalizedTitle?.startsWith("read")) return "read"
	if (normalizedTitle?.startsWith("edit")) return "edit"
	if (normalizedTitle?.startsWith("write")) return "write"
	if (normalizedTitle?.startsWith("search")) return "grep"
	if (normalizedTitle?.startsWith("fetch")) return "webfetch"
	if (normalizedTitle?.startsWith("run") || normalizedTitle?.startsWith("execute")) return "bash"
	return "tool"
}

function rawString(value: unknown): string {
	if (typeof value === "string") return value
	if (value === undefined || value === null) return ""
	return JSON.stringify(value)
}

function toolOutputString(update: Record<string, unknown>): string {
	if ("rawOutput" in update) return rawString(update.rawOutput)
	const contentOutput = outputFromAcpToolContent(update.content)
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

function outputFromAcpToolContent(content: unknown): string {
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
}> {
	if (!Array.isArray(options)) return []
	const result: Array<{ value: string; name: string; description?: string }> = []
	for (const option of options) {
		if (!option || typeof option !== "object") continue
		const value = (option as Record<string, unknown>).value
		const nestedOptions = (option as Record<string, unknown>).options
		if (typeof value === "string") {
			result.push({
				value,
				name: String((option as Record<string, unknown>).name ?? value),
				description:
					typeof (option as Record<string, unknown>).description === "string"
						? String((option as Record<string, unknown>).description)
						: undefined,
			})
			continue
		}
		result.push(...flattenSelectOptions(nestedOptions))
	}
	return result
}

function variantsFromConfigOption(option?: AcpConfigOption): Record<string, { name: string; description?: string }> {
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
