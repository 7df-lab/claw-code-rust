// @ts-nocheck

import {
	AsyncEventQueue,
	type AcpConfigOption,
	configDataFromConfigOptions,
	createIpcTransport,
	defaultCwd,
	permissionOptionId,
	partTime,
	providerDataFromConfigOptions,
	questionInfoFromAcp,
	requestUserInputFromOriginalEvent,
	sessionErrorEvent,
	stableId,
	statusFromDevo,
	textFromUpdate,
	toolCallIdFromUpdate,
	toolPartFromUpdate,
} from "./acp-client-support"
import type {
	AcpDeleteSessionParams,
	AcpListSessionsResult,
	AcpLoadSessionResult,
	AcpNewSessionResult,
	AcpPromptParams,
	AcpRequestPermissionParams,
	AcpSessionInfo,
	AcpSessionNotification,
	AcpSetConfigOptionParams,
	AcpSetConfigOptionResult,
} from "./generated"
import type {
	ProviderValidateParams,
	ProviderValidateResult,
	ProviderVendorListResult,
	ProviderVendorUpsertParams,
	ProviderVendorUpsertResult,
	InputItem,
	TurnStartResult,
	WorkspaceChangeCoverage,
	WorkspaceChangeScope,
	WorkspaceChangeSetStatus,
	WorkspaceChangeStats,
	WorkspaceChangeViewStatus,
	WorkspaceChangesReadParams,
	WorkspaceChangesReadResult,
	WorkspaceChangesUpdatedPayload,
	WorkspaceDiffDetail,
	SessionInterruptParams,
} from "./generated/protocol"
import {
	ProtocolValidationError,
	assertValidProtocolPayload,
} from "./protocol-validation"
import {
	ReferenceSearchSession,
	type ReferenceSearchSnapshot,
} from "./reference-search-session"

export type {
	ReferenceSearchResult,
	ReferenceSearchSnapshot,
} from "./reference-search-session"

export type JsonRpcId = number | string

export interface DevoAcpTransportEvent {
	type: "notification" | "request" | "closed"
	id?: JsonRpcId
	method?: string
	params?: unknown
	error?: string
}

export interface DevoAcpTransport {
	request(method: string, params?: unknown, directory?: string): Promise<unknown>
	notify?(method: string, params?: unknown, directory?: string): Promise<void>
	respond(id: JsonRpcId, result: unknown): Promise<void>
	subscribe(listener: (event: DevoAcpTransportEvent) => void): () => void
	connected(): boolean
}

export interface CreateDevoClientOptions {
	baseUrl?: string
	directory?: string
	fetch?: typeof fetch
	transport?: DevoAcpTransport
}

export type Agent = any
export type AgentConfig = any
export type AgentPart = any
export type AssistantMessage = any
export type Command = any
export type CompactionPart = any
export type Config = any
export type Event = any
export type EventMessagePartDelta = any
export type EventMessagePartUpdated = any
export type EventPermissionAsked = any
export type EventSessionCreated = any
export type EventSessionDeleted = any
export type EventSessionError = any
export type EventSessionStatus = any
export type EventSessionUpdated = any
export type FileDiff = any
export type FilePart = any
export type FilePartInput = any
export type McpLocalConfig = any
export type McpOAuthConfig = any
export type McpRemoteConfig = any
export type Message = any
export type Model = any
export type Part = any
export type PatchPart = any
export type PermissionAction = any
export type PermissionActionConfig = any
export type PermissionConfig = any
export type PermissionObjectConfig = any
export type PermissionRequest = any
export type PermissionRule = any
export type PermissionRuleConfig = any
export type PermissionRuleset = any
export type Project = any
export type Provider = any
export type ProviderAuthMethod = any
export type ProviderConfig = any
export type QuestionOption = {
	label: string
	description: string
}

export type QuestionInfo = {
	id: string
	header: string
	question: string
	options: QuestionOption[]
	isOther: boolean
	isSecret: boolean
}

export type QuestionRequest = {
	id: string
	requestID: string
	sessionID: string
	questions: QuestionInfo[]
}

export type QuestionAnswer = string[]
export type ReasoningPart = any
export type RetryPart = any
export type ServerConfig = any
export type Session = any
export type SessionStatus = any
export type SnapshotPart = any
export type StepFinishPart = any
export type StepStartPart = any
export type SubtaskPart = any
export type TextPart = any
export type Todo = any
export type ToolPart = any
export type ToolState = any
export type ToolStateCompleted = any
export type UserMessage = any
export type Worktree = any
export type {
	ProviderModelBinding,
	ProviderValidateParams,
	ProviderValidateResult,
	ProviderVendor,
	ProviderVendorListResult,
	ProviderVendorUpsertParams,
	ProviderVendorUpsertResult,
	ProviderWireApi,
	WorkspaceChangeAttribution,
	WorkspaceChangeBase,
	WorkspaceChangeCoverage,
	WorkspaceChangeScope,
	WorkspaceChangeSetStatus,
	WorkspaceChangeStats,
	WorkspaceChangeView,
	WorkspaceChangeViewStatus,
	WorkspaceChangedFile,
	WorkspaceChangedFileStatus,
	WorkspaceChangesReadParams,
	WorkspaceChangesReadResult,
	WorkspaceChangesUpdatedPayload,
	WorkspaceDiffDetail,
} from "./generated/protocol"

export type WorkspaceChangesReadOptions = {
	sessionID: string
	cwd?: string
	scopes: WorkspaceChangeScope[]
	baseBranch?: string
	turnID?: string
	diffDetail?: WorkspaceDiffDetail
	maxDiffBytes?: number | bigint
}

export type WorkspaceChangesUpdatedEventProperties = {
	sessionID: string
	turnID: string
	scope: WorkspaceChangeScope
	status: WorkspaceChangeViewStatus
	coverage: WorkspaceChangeCoverage
	changeSetStatus: WorkspaceChangeSetStatus
	stats: {
		filesChanged: number
		additions: number
		deletions: number
	}
	version: number
	generatedAt: string
}

interface GlobalEvent {
	directory: string
	payload: Event
}

type PendingQuestion = {
	id?: JsonRpcId
	sessionId: string
	questions: QuestionInfo[]
}

type PendingPermission = {
	id: JsonRpcId
	sessionId: string
	options: Array<{ optionId: string; kind: string }>
}

function partCacheKey(sessionId: string, messageId: string): string {
	return `${sessionId}\u001f${messageId}`
}

function objectRecord(value: unknown): Record<string, unknown> | undefined {
	return value && typeof value === "object" ? (value as Record<string, unknown>) : undefined
}

function sessionMeta(value: unknown): Record<string, unknown> | undefined {
	const meta = objectRecord(value)
	return objectRecord(meta?.["devo/session"])
}

function providerRetryStatusFromOriginalEvent(
	original: Record<string, unknown>,
	originalMethod?: string,
): Record<string, unknown> | null {
	if (originalMethod !== "turn/provider_retry_status" && !("TurnProviderRetryStatus" in original) && original.kind !== "turn_provider_retry_status") {
		return null
	}
	const payload = objectRecord(original.TurnProviderRetryStatus) ?? original
	const sessionID = String(payload.session_id ?? payload.sessionId ?? "")
	const turnID = String(payload.turn_id ?? payload.turnId ?? "")
	if (!sessionID || !turnID) return null
	return {
		sessionID,
		turnID,
		attempt: numberFromProtocol(payload.attempt),
		backoffMs: numberFromProtocol(payload.backoff_ms ?? payload.backoffMs),
		provider: String(payload.provider ?? ""),
		model: String(payload.model ?? ""),
		phase: String(payload.phase ?? ""),
		message: String(payload.message ?? ""),
	}
}

function turnFailureFromOriginalEvent(
	original: Record<string, unknown>,
	originalMethod?: string,
): { sessionID: string; code: string; message: string } | null {
	if (originalMethod !== "turn/failed" && !("TurnFailed" in original) && original.kind !== "turn_failed") {
		return null
	}
	const payload = objectRecord(original.TurnFailed) ?? original
	const sessionID = String(payload.session_id ?? payload.sessionId ?? "")
	if (!sessionID) return null
	const error = objectRecord(payload.error)
	if (!error || typeof error.message !== "string" || !error.message.trim()) return null
	return {
		sessionID,
		code: String(error.code ?? "TURN_FAILED"),
		message: error.message,
	}
}

function sessionStatusFromMetadata(value: unknown): string | undefined {
	const meta = objectRecord(value)
	const nestedStatus = objectRecord(meta?.["devo/session"])?.status
	if (typeof nestedStatus === "string") return nestedStatus
	const directStatus = meta?.["devo/session.status"]
	return typeof directStatus === "string" ? directStatus : undefined
}

function numberFromProtocol(value: unknown): number {
	if (typeof value === "number" && Number.isFinite(value)) return value
	if (typeof value === "bigint") return Number(value)
	if (typeof value === "string") {
		const parsed = Number(value)
		if (Number.isFinite(parsed)) return parsed
	}
	return 0
}

function workspaceChangeStats(value: unknown): WorkspaceChangeStats {
	const stats = objectRecord(value)
	return {
		files_changed: numberFromProtocol(stats?.files_changed ?? stats?.filesChanged),
		additions: numberFromProtocol(stats?.additions),
		deletions: numberFromProtocol(stats?.deletions),
	}
}

function workspaceChangesUpdatedFromOriginalEvent(
	original: unknown,
): WorkspaceChangesUpdatedPayload | null {
	const event = objectRecord(original)
	if (!event) return null
	const payload =
		event.kind === "workspace_changes_updated"
			? event
			: objectRecord(event.WorkspaceChangesUpdated) ??
				objectRecord(event.workspace_changes_updated)
	if (!payload) return null
	return {
		session_id: String(payload.session_id ?? payload.sessionId ?? ""),
		turn_id: String(payload.turn_id ?? payload.turnId ?? ""),
		scope: String(payload.scope ?? "turn") as WorkspaceChangeScope,
		status: String(payload.status ?? "ready") as WorkspaceChangeViewStatus,
		coverage: String(payload.coverage ?? "none") as WorkspaceChangeCoverage,
		change_set_status: String(
			payload.change_set_status ?? payload.changeSetStatus ?? "finalized",
		) as WorkspaceChangeSetStatus,
		stats: workspaceChangeStats(payload.stats),
		version: numberFromProtocol(payload.version),
		generated_at: String(payload.generated_at ?? payload.generatedAt ?? ""),
	}
}

// ── Canonical provider conversions (ratified #11) ──

function canonicalProviderVendorWire(vendor: ProviderVendor): Record<string, unknown> {
	return {
		name: vendor.name,
		...(vendor.base_url != null ? { baseUrl: vendor.base_url } : {}),
		...(vendor.credential != null ? { credential: vendor.credential } : {}),
		...(vendor.headers != null ? { headers: vendor.headers } : {}),
		wireApis: vendor.wire_apis,
		enabled: vendor.enabled,
	}
}

function canonicalModelBindingWire(binding: ProviderModelBinding): Record<string, unknown> {
	return {
		bindingId: binding.binding_id,
		modelSlug: binding.model_slug,
		provider: binding.provider,
		requestModel: binding.request_model,
		...(binding.display_name != null ? { displayName: binding.display_name } : {}),
		invocationMethod: binding.invocation_method,
		...(binding.default_reasoning_effort != null
			? { defaultReasoningEffort: binding.default_reasoning_effort }
			: {}),
		enabled: binding.enabled,
	}
}

function legacyProviderVendorFromCanonical(vendor: Record<string, unknown>): ProviderVendor {
	return {
		name: String(vendor.name ?? ""),
		base_url: (vendor.baseUrl as string | null) ?? null,
		credential: (vendor.credential as string | null) ?? null,
		headers: (vendor.headers as string | null) ?? null,
		wire_apis: (vendor.wireApis ?? []) as ProviderVendor["wire_apis"],
		enabled: Boolean(vendor.enabled),
	}
}

function legacyModelBindingFromCanonical(binding: Record<string, unknown>): ProviderModelBinding {
	return {
		binding_id: String(binding.bindingId ?? ""),
		model_slug: String(binding.modelSlug ?? ""),
		provider: String(binding.provider ?? ""),
		request_model: String(binding.requestModel ?? ""),
		display_name: (binding.displayName as string | null) ?? null,
		invocation_method: binding.invocationMethod as ProviderModelBinding["invocation_method"],
		default_reasoning_effort: (binding.defaultReasoningEffort as string | null) ?? null,
		enabled: Boolean(binding.enabled),
	}
}

/** Canonical `model/preferences` wire shape (ratified #12). */type ModelPreferencesWire = {
	model?: string
	reasoningEffort?: string
	availableModels?: Array<{ value: string; label: string; description?: string }>
	availableEfforts?: Array<{ value: string; label: string; description?: string }>
}

/** Canonical model preferences → the ACP select options the config UI renders. */
function acpConfigOptionsFromModelPreferences(preferences: ModelPreferencesWire): AcpConfigOption[] {
	const toSelectOptions = (entries?: Array<{ value: string; label: string; description?: string }>) =>
		(entries ?? []).map((entry) => ({
			value: entry.value,
			name: entry.label,
			...(entry.description !== undefined ? { description: entry.description } : {}),
		}))
	const options: AcpConfigOption[] = []
	if (preferences.model !== undefined || (preferences.availableModels?.length ?? 0) > 0) {
		options.push({
			type: "select",
			id: "model",
			name: "Model",
			description: "Controls the model used for this session",
			category: "model",
			currentValue: preferences.model ?? "",
			options: toSelectOptions(preferences.availableModels),
		} as AcpConfigOption)
	}
	if (preferences.reasoningEffort !== undefined || (preferences.availableEfforts?.length ?? 0) > 0) {
		options.push({
			type: "select",
			id: "thought_level",
			name: "Reasoning Effort",
			description: "Controls the model reasoning effort used for this session",
			category: "thought_level",
			currentValue: preferences.reasoningEffort ?? "",
			options: toSelectOptions(preferences.availableEfforts),
		} as AcpConfigOption)
	}
	return options
}

/** Canonical (camelCase) workspace change view → legacy generated shape. */
function legacyWorkspaceChangeViewFromCanonical(	view: Record<string, unknown>,
): WorkspaceChangeView {
	const base = objectRecord(view.base)
	return {
		scope: view.scope as WorkspaceChangeView["scope"],
		status: view.status as WorkspaceChangeView["status"],
		workspace_root: String(view.workspaceRoot ?? ""),
		base: base ? legacyWorkspaceChangeBaseFromCanonical(base) : undefined,
		coverage: view.coverage as WorkspaceChangeView["coverage"],
		attribution: view.attribution as WorkspaceChangeView["attribution"],
		change_set_status: view.changeSetStatus as WorkspaceChangeView["change_set_status"],
		files: (view.files ?? []) as WorkspaceChangeView["files"],
		stats: workspaceChangeStats(view.stats),
		unified_diff: typeof view.unifiedDiff === "string" ? view.unifiedDiff : undefined,
		warnings: (view.warnings ?? []) as string[],
		generated_at: String(view.generatedAt ?? ""),
	}
}

function legacyWorkspaceChangeBaseFromCanonical(
	base: Record<string, unknown>,
): WorkspaceChangeBase {
	if (base.kind === "branch") {
		return {
			kind: "branch",
			base_branch: String(base.baseBranch ?? ""),
			merge_base: String(base.mergeBase ?? ""),
			head: String(base.head ?? ""),
		} as WorkspaceChangeBase
	}
	if (base.kind === "turn_checkpoint") {
		return {
			kind: "turn_checkpoint",
			turn_id: String(base.turnId ?? ""),
			checkpoint_id: String(base.checkpointId ?? ""),
			backend: base.backend,
		} as WorkspaceChangeBase
	}
	return {
		kind: "head",
		head: typeof base.head === "string" ? base.head : undefined,
	} as WorkspaceChangeBase
}

function deletedSessionIdsFromOriginalEvent(original: unknown): string[] {
	const event = objectRecord(original)
	if (!event) return []
	const payload =
		event.kind === "session_deleted"
			? event
			: objectRecord(event.SessionDeleted) ?? objectRecord(event.session_deleted)
	if (!payload) return []
	const rawIds = payload.deleted_session_ids ?? payload.deletedSessionIds
	if (Array.isArray(rawIds)) return rawIds.map(String).filter(Boolean)
	const sessionId = payload.session_id ?? payload.sessionId
	return sessionId ? [String(sessionId)] : []
}

function sessionStatusChangedFromOriginalEvent(
	original: unknown,
	originalMethod?: string,
): { sessionId: string; status: string } | null {
	const event = objectRecord(original)
	if (!event) return null
	const payload =
		originalMethod === "session/status/changed"
			? objectRecord(event.SessionStatusChanged) ?? event
			: event.kind === "session_status_changed" || event.kind === "session/status/changed"
				? event
				: objectRecord(event.SessionStatusChanged) ??
					objectRecord(event.session_status_changed) ??
					objectRecord(event.sessionStatusChanged)
	if (!payload) return null
	const sessionId = payload.session_id ?? payload.sessionId
	const status = payload.status
	return typeof sessionId === "string" && typeof status === "string" ? { sessionId, status } : null
}

function sessionIdFromCompactionPayload(payload: Record<string, unknown>): string | null {
	const direct = payload.session_id ?? payload.sessionId
	if (typeof direct === "string" && direct) return direct
	const context = objectRecord(payload.context)
	const contextual = context?.session_id ?? context?.sessionId
	if (typeof contextual === "string" && contextual) return contextual
	const session = objectRecord(payload.session)
	const nested = session?.session_id ?? session?.sessionId
	return typeof nested === "string" && nested ? nested : null
}

function sessionCompactionFromOriginalEvent(
	original: unknown,
	originalMethod?: string,
): {
	sessionId: string
	status: "started" | "completed" | "failed"
	message?: string
	itemId?: string
	turnId?: string
} | null {
	const event = objectRecord(original)
	if (!event) return null

	let status: "started" | "completed" | "failed" | null = null
	let payload: Record<string, unknown> | undefined
	let itemId: string | undefined
	let turnId: string | undefined
	if (originalMethod === "item/started" || originalMethod === "item/completed") {
		const item = objectRecord(event.item)
		if (item?.item_kind !== "context_compaction" && item?.itemKind !== "context_compaction") {
			return null
		}
		const context = objectRecord(event.context)
		const itemPayload = objectRecord(item.payload)
		status = originalMethod === "item/started"
			? "started"
			: itemPayload?.status === "failed"
				? "failed"
				: "completed"
		payload = event
		const rawItemId = item.item_id ?? item.itemId
		const rawTurnId = context?.turn_id ?? context?.turnId
		itemId = typeof rawItemId === "string" && rawItemId ? rawItemId : undefined
		turnId = typeof rawTurnId === "string" && rawTurnId ? rawTurnId : undefined
	} else if (originalMethod === "session/compaction/started") {
		status = "started"
		payload = objectRecord(event.SessionCompactionStarted) ?? event
	} else if (originalMethod === "session/compaction/completed") {
		status = "completed"
		payload = objectRecord(event.SessionCompactionCompleted) ?? event
	} else if (originalMethod === "session/compaction/failed") {
		status = "failed"
		payload = objectRecord(event.SessionCompactionFailed) ?? event
	} else {
		const candidates: Array<
			["started" | "completed" | "failed", Record<string, unknown> | undefined]
		> = [
			["started", objectRecord(event.SessionCompactionStarted)],
			["started", objectRecord(event.session_compaction_started)],
			["started", objectRecord(event.sessionCompactionStarted)],
			["completed", objectRecord(event.SessionCompactionCompleted)],
			["completed", objectRecord(event.session_compaction_completed)],
			["completed", objectRecord(event.sessionCompactionCompleted)],
			["failed", objectRecord(event.SessionCompactionFailed)],
			["failed", objectRecord(event.session_compaction_failed)],
			["failed", objectRecord(event.sessionCompactionFailed)],
		]
		const found = candidates.find(([, value]) => value)
		if (found) {
			status = found[0]
			payload = found[1]
		} else if (event.kind === "session_compaction_started") {
			status = "started"
			payload = event
		} else if (event.kind === "session_compaction_completed") {
			status = "completed"
			payload = event
		} else if (event.kind === "session_compaction_failed") {
			status = "failed"
			payload = event
		}
	}

	if (!status || !payload) return null
	const sessionId = sessionIdFromCompactionPayload(payload)
	if (!sessionId) return null
	const itemPayload = objectRecord(objectRecord(payload.item)?.payload)
	const message = payload.message ?? itemPayload?.message
	return {
		sessionId,
		status,
		...(typeof message === "string" && message ? { message } : {}),
		...(itemId ? { itemId } : {}),
		...(turnId ? { turnId } : {}),
	}
}

function workspaceChangesUpdatedEventProperties(
	payload: WorkspaceChangesUpdatedPayload,
): WorkspaceChangesUpdatedEventProperties {
	return {
		sessionID: payload.session_id,
		turnID: payload.turn_id,
		scope: payload.scope,
		status: payload.status,
		coverage: payload.coverage,
		changeSetStatus: payload.change_set_status,
		stats: {
			filesChanged: numberFromProtocol(payload.stats.files_changed),
			additions: numberFromProtocol(payload.stats.additions),
			deletions: numberFromProtocol(payload.stats.deletions),
		},
		version: numberFromProtocol(payload.version),
		generatedAt: payload.generated_at,
	}
}

function parseTimestampMs(value: unknown): number | undefined {
	if (typeof value !== "string") return undefined
	const parsed = Date.parse(value)
	return Number.isFinite(parsed) ? parsed : undefined
}

type LoadedSessionLimit = number | null
const HISTORY_MESSAGE_ID_RE = /^(?:tool-)?history-(\d+)$/
const DEVO_TURN_ID_META = "devo/turnId"
const DEVO_ACTIVITY_AT_META = "devo/activityAt"
const DEVO_HISTORY_INDEX_META = "devo/historyIndex"
const DEVO_PARENT_MESSAGE_ID_META = "devo/parentMessageId"
const DEVO_TURN_DURATION_MS_META = "devo/turnDurationMs"
const DEVO_ITEM_KIND_META = "devo/itemKind"
const DEVO_RESEARCH_ARTIFACT_TYPE_META = "devo/researchArtifactType"
const DEVO_RESEARCH_ARTIFACT_TITLE_META = "devo/researchArtifactTitle"

type PromptPartInput = {
	type: string
	text?: string
	url?: string
	filename?: string
	mime?: string
	mediaType?: string
}

function pathFromFileUri(uri: string): string | null {
	if (!uri.startsWith("file://")) return null
	try {
		const url = new URL(uri)
		let path = decodeURIComponent(url.pathname)
		if (/^\/[A-Za-z]:/.test(path)) path = path.slice(1)
		return path.replace(/\//g, "\\")
	} catch {
		return uri.slice("file://".length)
	}
}

function inputItemsFromPromptParts(parts: PromptPartInput[]): InputItem[] {
	const input: InputItem[] = []
	const text = parts
		.map((part) => (part.type === "text" ? (part.text ?? "") : ""))
		.join("\n")
		.trim()
	if (text || parts.every((part) => part.type !== "file")) {
		input.push({ type: "text", text })
	}
	for (const part of parts) {
		if (part.type !== "file" || !part.url) continue
		const path = pathFromFileUri(part.url)
		if (path) {
			input.push({
				type: "mention",
				path,
				name: part.filename ?? path.split(/[\\/]/).pop() ?? path,
			})
			continue
		}
		input.push({
			type: "text",
			text: `Resource ${part.filename ?? part.url}: ${part.url}`,
		})
	}
	return input
}

function normalizedHistoryLimit(limit: unknown): number | undefined {
	if (typeof limit !== "number" || !Number.isFinite(limit) || limit <= 0) return undefined
	return Math.floor(limit)
}

function loadedLimitCovers(loaded: LoadedSessionLimit | undefined, requested: number | undefined): boolean {
	if (loaded === undefined) return false
	if (loaded === null) return true
	return requested !== undefined && loaded >= requested
}

function historyMessageCreatedAt(messageId: string): number | undefined {
	const match = HISTORY_MESSAGE_ID_RE.exec(messageId)
	if (!match) return undefined
	const index = Number.parseInt(match[1], 10)
	return Number.isFinite(index) ? index + 1 : undefined
}

function updateMeta(update: Record<string, unknown>): Record<string, unknown> | undefined {
	return objectRecord(update._meta)
}

function updateMetaString(update: Record<string, unknown>, key: string): string | undefined {
	const value = updateMeta(update)?.[key]
	return typeof value === "string" && value ? value : undefined
}

function textPartMetadataFromUpdate(
	update: Record<string, unknown>,
	existingPart?: Record<string, unknown>,
): Record<string, unknown> | undefined {
	const existing = objectRecord(existingPart?.metadata)
	const metadata = existing ? { ...existing } : {}
	const meta = updateMeta(update)
	if (meta?.[DEVO_ITEM_KIND_META] === "research_artifact") {
		metadata[DEVO_ITEM_KIND_META] = "research_artifact"
		const artifactType = meta[DEVO_RESEARCH_ARTIFACT_TYPE_META]
		if (typeof artifactType === "string" && artifactType) {
			metadata[DEVO_RESEARCH_ARTIFACT_TYPE_META] = artifactType
		}
		const title = meta[DEVO_RESEARCH_ARTIFACT_TITLE_META]
		if (typeof title === "string" && title) {
			metadata[DEVO_RESEARCH_ARTIFACT_TITLE_META] = title
		}
	}
	return Object.keys(metadata).length > 0 ? metadata : undefined
}

function updateHistoryCreatedAt(update: Record<string, unknown>): number | undefined {
	const value = updateMeta(update)?.[DEVO_HISTORY_INDEX_META]
	const index =
		typeof value === "number"
			? value
			: typeof value === "string"
				? Number.parseInt(value, 10)
				: undefined
	return typeof index === "number" && Number.isFinite(index) && index >= 0
		? Math.floor(index) + 1
		: undefined
}

function messageCreatedAt(message: Message): number {
	const historyCreated = historyMessageCreatedAt(message.id)
	if (historyCreated !== undefined) return historyCreated
	const created = message.time?.created
	return typeof created === "number" && Number.isFinite(created) ? created : 0
}

function compareMessages(left: Message, right: Message): number {
	const byCreated = messageCreatedAt(left) - messageCreatedAt(right)
	return byCreated === 0 ? left.id.localeCompare(right.id) : byCreated
}

function sortedMessages(messages: Message[]): Message[] {
	return [...messages].sort(compareMessages)
}

const initializePromises = new WeakMap<DevoAcpTransport, Promise<void>>()

const DESKTOP_INITIALIZE_PARAMS = {
	protocolVersion: 1,
	clientCapabilities: {
		fs: { readTextFile: false, writeTextFile: false },
		terminal: false,
	},
	clientInfo: {
		name: "devo-desktop",
		title: "Devo Desktop",
		version: "0.1.0",
	},
} as const

function recentMessages(messages: Message[], limit: number | undefined): Message[] {
	const sorted = sortedMessages(messages)
	if (limit === undefined || sorted.length <= limit) return sorted
	let start = sorted.length - limit
	while (start > 0 && sorted[start].role !== "user") {
		start -= 1
	}
	return sorted.slice(start)
}

class AcpClient {
	private transport: DevoAcpTransport | null = null
	private openPromise: Promise<void> | null = null
	private initialized = false
	private events = new AsyncEventQueue<GlobalEvent>()
	private sessions = new Map<string, Session>()
	private sessionDirectories = new Map<string, string>()
	private sessionStatuses = new Map<string, SessionStatus>()
	private messages = new Map<string, Message[]>()
	private parts = new Map<string, Part[]>()
	private loadedSessionLimits = new Map<string, LoadedSessionLimit>()
	private lastUserMessageBySession = new Map<string, string>()
	private userMessageByTurn = new Map<string, string>()
	private messageTurnIds = new Map<string, string>()
	private configOptionsBySession = new Map<string, AcpConfigOption[]>()
	private configOptionsByDirectory = new Map<string, AcpConfigOption[]>()
	private pendingPermissions = new Map<string, PendingPermission>()
	private pendingQuestions = new Map<string, PendingQuestion>()
	private sessionDiscovery = new Map<string, Promise<Session | undefined>>()
	private lastEventTime = 0
	private referenceSearchSession: ReferenceSearchSession | null = null

	constructor(private readonly options: CreateDevoClientOptions) {}
	project = {
		list: async () => ({ data: await this.listProjects() }),
	}
	session = {
		list: async (params?: { limit?: number; roots?: boolean; search?: string }) => ({
			data: await this.listSessions(params),
		}),
		status: async () => ({ data: Object.fromEntries(this.sessionStatuses) }),
		create: async (_params?: { title?: string }) => ({ data: await this.createSession() }),
		promptAsync: async (params: {
			sessionID: string
			parts: PromptPartInput[]
			model?: unknown
			agent?: string
			variant?: string
			collaborationMode?: string
		}) => {
			// Plan mode uses turn/start which supports collaboration_mode
			if (params.collaborationMode === "plan") {
				const result = await this.turn.start({
					sessionID: params.sessionID,
					parts: params.parts,
					model: params.model,
					variant: params.variant,
					collaborationMode: "plan",
				})
				return result
			}
			const model = params.model as { modelID?: string } | undefined
			if (model?.modelID) await this.setSessionConfigOption(params.sessionID, "model", model.modelID)
			if (params.variant) await this.setSessionConfigOption(params.sessionID, "thought_level", params.variant)
			const text = params.parts
				.map((part) => (part.type === "text" ? (part.text ?? "") : ""))
				.join("\n")
				.trim()
			const prompt = []
			if (text || params.parts.every((part) => part.type !== "file")) {
				prompt.push({ type: "text", text })
			}
			for (const part of params.parts) {
				if (part.type !== "file" || !part.url) continue
				prompt.push({
					type: "resource_link",
					uri: part.url,
					name: part.filename ?? part.url,
					...(part.mime || part.mediaType ? { mimeType: part.mime ?? part.mediaType } : {}),
				})
			}
			const promptParams: AcpPromptParams = {
				sessionId: params.sessionID,
				prompt,
			}
			const directory = this.sessionDirectories.get(params.sessionID) ?? this.options.directory ?? defaultCwd()
			this.lastUserMessageBySession.delete(params.sessionID)
			const promptStartedAt = Math.max(Date.now(), this.lastEventTime + 1)
			const busyStatus = { type: "busy" }
			this.sessionStatuses.set(params.sessionID, busyStatus)
			this.emit(directory, {
				type: "session.status",
				properties: { sessionID: params.sessionID, status: busyStatus },
			})
			void this.request("session/prompt", promptParams)
				.then(() => {
					this.completeOpenAssistantMessages(params.sessionID, directory, promptStartedAt)
					const idleStatus = { type: "idle" }
					this.sessionStatuses.set(params.sessionID, idleStatus)
					this.emit(directory, {
						type: "session.status",
						properties: { sessionID: params.sessionID, status: idleStatus },
					})
				})
				.catch((error) => {
					this.completeOpenAssistantMessages(params.sessionID, directory, promptStartedAt)
					const idleStatus = { type: "idle" }
					this.sessionStatuses.set(params.sessionID, idleStatus)
					this.emit(directory, sessionErrorEvent(params.sessionID, error))
					this.emit(directory, {
						type: "session.status",
						properties: { sessionID: params.sessionID, status: idleStatus },
					})
				})
		},
		abort: async (params: { sessionID: string }) => {
			const interruptParams: SessionInterruptParams = {
				scope: { scope: "session", sessionId: params.sessionID },
			}
			await this.request("session/interrupt", interruptParams)
		},
		update: async (params: { sessionID: string; title: string }) => {
			// Canonical session/metadata/update (L2-DES-APP-008): the title
			// patch on the persist-first path; result is the canonical Session.
			const result = (await this.requestCanonical("session/metadata/update", {
				sessionId: params.sessionID,
				expectedVersion: 0,
				title: params.title,
			})) as { session?: Record<string, unknown> }
			const metadata = result.session ?? {}
			const usage = objectRecord(metadata.usage)
			const usageTotals = objectRecord(usage?.total)
			// `rememberSession` folds the legacy snake-shaped devo/session
			// metadata; rebuild it from the canonical Session so downstream
			// session fields (timestamps, token totals) keep their meaning.
			const legacyMeta: Record<string, unknown> = {
				session_id: String(metadata.id ?? params.sessionID),
				cwd: metadata.cwd,
				created_at: metadata.createdAt,
				updated_at: metadata.lastActivityAt,
				last_activity_at: metadata.lastActivityAt,
				title: metadata.title,
				ephemeral: metadata.ephemeral ?? false,
				status: metadata.status,
				total_input_tokens: usageTotals?.inputTokens ?? 0,
				total_output_tokens: usageTotals?.outputTokens ?? 0,
				total_tokens: usageTotals?.totalTokens ?? 0,
				total_cache_creation_tokens: usageTotals?.cacheCreationInputTokens ?? 0,
				total_cache_read_tokens: usageTotals?.cacheReadInputTokens ?? 0,
				prompt_token_estimate: usageTotals?.inputTokens ?? 0,
			}
			const session = this.rememberSession({
				sessionId: String(metadata.id ?? params.sessionID),
				cwd: String(metadata.cwd ?? this.sessionDirectories.get(params.sessionID) ?? this.options.directory ?? defaultCwd()),
				title: typeof metadata.title === "string" ? metadata.title : params.title,
				updatedAt: typeof metadata.lastActivityAt === "string" ? metadata.lastActivityAt : undefined,
				_meta: { "devo/session": legacyMeta },
			})
			this.emit(session.directory ?? this.options.directory ?? defaultCwd(), {
				type: "session.updated",
				properties: { info: session, session },
			})
			return { data: session }
		},
		delete: async (params: { sessionID: string }) => {
			const deleteParams: AcpDeleteSessionParams = { sessionId: params.sessionID }
			await this.request("session/delete", deleteParams)
			const { directory } = this.forgetSession(params.sessionID)
			this.emitSessionDeleted(params.sessionID, directory)
		},
		get: async (params: { sessionID: string }) => ({
			data: await this.getSessionById(params.sessionID),
		}),
		diff: async (_params: { sessionID: string }) => ({ data: [] }),
		revert: async (params: { sessionID: string }) => ({
			data: this.sessions.get(params.sessionID),
		}),
		unrevert: async (params: { sessionID: string }) => ({
			data: this.sessions.get(params.sessionID),
		}),
		command: async (params: { sessionID: string; command: string; arguments?: string }) => {
			const suffix = params.arguments ? ` ${params.arguments}` : ""
			await this.session.promptAsync({
				sessionID: params.sessionID,
				parts: [{ type: "text", text: `/${params.command}${suffix}` }],
			})
		},
		summarize: async (params: { sessionID: string }) => {
			await this.session.promptAsync({
				sessionID: params.sessionID,
				parts: [{ type: "text", text: "/compact" }],
			})
		},
		messages: async (params: { sessionID: string; limit?: number }) => ({
			data: await this.sessionMessages(params.sessionID, normalizedHistoryLimit(params.limit)),
		}),
		fork: async (params: { sessionID: string }) => ({
			data: this.sessions.get(params.sessionID),
		}),
	}

	turn = {
		start: async (params: {
			sessionID: string
			parts: PromptPartInput[]
			model?: unknown
			variant?: string
			cwd?: string | null
			collaborationMode?: string
		}) => {
			const model = params.model as { modelID?: string } | undefined
			if (model?.modelID) await this.setSessionConfigOption(params.sessionID, "model", model.modelID)
			if (params.variant) await this.setSessionConfigOption(params.sessionID, "thought_level", params.variant)
			const result = (await this.request("turn/start", {
				session_id: params.sessionID,
				input: inputItemsFromPromptParts(params.parts),
				model_binding_id: model?.modelID ?? null,
				sandbox: null,
				approval_policy: null,
				cwd: params.cwd ?? null,
				collaboration_mode: params.collaborationMode ?? "build",
			})) as TurnStartResult
			return { data: result }
		},
	}

	question = {
		reply: async (params: { requestID: string; answers: QuestionAnswer[] }) => {
			await this.respondToQuestion(params.requestID, params.answers, "question.replied")
		},
		reject: async (params: { requestID: string }) => {
			await this.respondToQuestion(params.requestID, [], "question.rejected")
		},
	}

	permission = {
		respond: async (params: {
			sessionID: string
			permissionID: string
			response: "once" | "always" | "reject"
		}) => {
			await this.respondToPermission(params.permissionID, params.response)
		},
		reply: async (params: { requestID: string; reply?: "once" | "always" | "reject" }) => {
			await this.respondToPermission(params.requestID, params.reply ?? "reject")
		},
	}

	instance = {
		dispose: async () => {},
	}

	global = {
		dispose: async () => {},
		event: async () => {
			await this.ensureInitialized()
			return { stream: this.events }
		},
		config: {
			update: async (_params: unknown) => ({ data: null }),
		},
	}

	event = {
		subscribe: async () => {
			await this.ensureInitialized()
			return { stream: this.events }
		},
	}

	workspace = {
		changes: {
			read: async (params: WorkspaceChangesReadOptions) => {
				// Canonical workspace/changes/read (L2-DES-APP-008): camelCase
				// wire shape; views convert back to the legacy snake shape the
				// renderer consumes while it stays on generated bindings.
				const wireParams: Record<string, unknown> = {
					sessionId: params.sessionID,
					scopes: params.scopes,
					diffDetail: params.diffDetail ?? "summary",
				}
				if (params.cwd !== undefined) wireParams.cwd = params.cwd
				if (params.baseBranch !== undefined) wireParams.baseBranch = params.baseBranch
				if (params.turnID !== undefined) wireParams.turnId = params.turnID
				if (params.maxDiffBytes !== undefined) {
					wireParams.maxDiffBytes = Number(params.maxDiffBytes)
				}
				const canonical = (await this.requestCanonical(
					"workspace/changes/read",
					wireParams,
				)) as { views?: Array<Record<string, unknown>> }
				const data: WorkspaceChangesReadResult = {
					views: (canonical.views ?? []).map(legacyWorkspaceChangeViewFromCanonical),
				}
				return { data }
			},
		},
	}

	command = {
		list: async () => ({ data: [{ name: "compact", description: "Compact the session" }] }),
	}

	// User requirement: Desktop's composer status area needs direct goal state
	// controls, while the existing /goal trigger remains available for entry.
	private async canonicalGoalTransition(method: string, sessionID: string): Promise<unknown> {
		const current = (await this.requestCanonical("session/goal/read", {
			sessionId: sessionID,
		})) as { goal?: { id?: string } | null }
		const expectedGoalId = current.goal?.id
		if (!expectedGoalId) throw new Error("session has no active goal")
		return this.requestCanonical(method, {
			sessionId: sessionID,
			expectedGoalId,
		})
	}

	goal = {
		status: async (params: { sessionID: string }) => {
			const result = (await this.requestCanonical("session/goal/read", {
				sessionId: params.sessionID,
			})) as { goal?: unknown }
			return { data: result.goal }
		},
		pause: async (params: { sessionID: string }) => {
			const result = (await this.canonicalGoalTransition(
				"session/goal/pause",
				params.sessionID,
			)) as { goal?: unknown }
			return { data: result.goal }
		},
		resume: async (params: { sessionID: string }) => {
			const result = (await this.canonicalGoalTransition(
				"session/goal/resume",
				params.sessionID,
			)) as { goal?: unknown }
			return { data: result.goal }
		},
		clear: async (params: { sessionID: string }) => {
			const result = await this.canonicalGoalTransition(
				"session/goal/clear",
				params.sessionID,
			)
			return { data: result }
		},
	}

	find = {
		// @ mention file search uses connection-local search/* RPC + notifications.
		files: async (params: { query: string }) => {
			const session = this.ensureReferenceSearchSession()
			await session.startOrUpdate(params.query)
			return { data: session.filePaths() }
		},
	}

	referenceSearch = {
		startOrUpdate: async (params: { query: string }) => ({
			data: await this.ensureReferenceSearchSession().startOrUpdate(params.query),
		}),
		cancel: async () => {
			await this.ensureReferenceSearchSession().cancel()
			return { data: null }
		},
		subscribe: (listener: (snapshot: ReferenceSearchSnapshot) => void) =>
			this.ensureReferenceSearchSession().subscribe(listener),
		getState: () => this.ensureReferenceSearchSession().getState(),
	}

	worktree = {
		list: async () => ({ data: [] }),
		create: async (_params: unknown) => ({ data: null }),
		remove: async (_params: unknown) => ({ data: null }),
		reset: async (_params: unknown) => ({ data: null }),
	}

	config = {
		providers: async () => ({
			data: providerDataFromConfigOptions(await this.ensureCurrentConfigOptions()),
		}),
		get: async () => ({ data: configDataFromConfigOptions(await this.ensureCurrentConfigOptions()) }),
		setOption: async (params: { configID: string; value: string }) => ({
			data: configDataFromConfigOptions(
				await this.setDefaultConfigOption(params.configID, params.value),
			),
		}),
	}

	vcs = {
		get: async () => ({ data: null }),
	}

	app = {
		agents: async () => ({ data: [] }),
		skills: async () => ({ data: [] }),
	}

	provider = {
		list: async () => {
			// Canonical provider/list (ratified #11): camelCase wire; vendors
			// convert back to the generated snake shape for callers.
			const result = (await this.requestCanonical("provider/list", {})) as {
				providers?: Array<Record<string, unknown>>
			}
			const data: ProviderVendorListResult = {
				provider_vendors: (result.providers ?? []).map(legacyProviderVendorFromCanonical),
			}
			return { data }
		},
		validate: async (params: ProviderValidateParams) => {
			const result = (await this.requestCanonical("provider/validate", {
				providerVendor: canonicalProviderVendorWire(params.provider_vendor),
				modelBinding: canonicalModelBindingWire(params.model_binding),
				...(params.api_key !== undefined && params.api_key !== null
					? { apiKey: params.api_key }
					: {}),
			})) as { replyPreview?: string }
			const data: ProviderValidateResult = { reply_preview: result.replyPreview ?? "" }
			return { data }
		},
		upsert: async (params: ProviderVendorUpsertParams) => {
			const result = (await this.requestCanonical("provider/upsert", {
				providerVendor: canonicalProviderVendorWire(params.provider_vendor),
				...(params.model_binding
					? { modelBinding: canonicalModelBindingWire(params.model_binding) }
					: {}),
				...(params.default_model_binding !== undefined && params.default_model_binding !== null
					? { defaultModelBinding: params.default_model_binding }
					: {}),
				...(params.api_key !== undefined && params.api_key !== null
					? { apiKey: params.api_key }
					: {}),
			})) as {
				providerVendor?: Record<string, unknown>
				modelBinding?: Record<string, unknown>
			}
			const data: ProviderVendorUpsertResult = {
				provider_vendor: legacyProviderVendorFromCanonical(result.providerVendor ?? {}),
				...(result.modelBinding
					? { model_binding: legacyModelBindingFromCanonical(result.modelBinding) }
					: {}),
			} as ProviderVendorUpsertResult
			this.invalidateConfigOptionCaches()
			return { data }
		},
		auth: async () => ({ data: [] }),
		oauth: {
			authorize: async (_params: unknown) => ({ data: null }),
			callback: async (_params: unknown) => ({ data: null }),
		},
	}

	auth = {
		set: async (_params: unknown) => ({ data: null }),
		remove: async (_params: unknown) => ({ data: null }),
	}

	part = {
		delete: async (_params: unknown) => ({ data: null }),
	}

	private async listProjects(): Promise<Project[]> {
		const sessions = await this.listSessions()
		const byDirectory = new Map<string, Project>()
		for (const session of sessions) {
			const directory = session.directory ?? this.options.directory
			if (!directory) continue
			const previous = byDirectory.get(directory)
			const updated = session.time.lastActivity ?? session.time.updated ?? session.time.created
			if (previous) {
				previous.time.updated = Math.max(previous.time.updated ?? 0, updated)
				continue
			}
			byDirectory.set(directory, {
				id: stableId(directory),
				name: directory.split(/[\\/]/).filter(Boolean).at(-1) ?? directory,
				worktree: directory,
				path: { root: directory },
				time: { created: session.time.created, updated },
				sandboxes: [],
			})
		}
		if (byDirectory.size === 0 && this.options.directory) {
			byDirectory.set(this.options.directory, {
				id: stableId(this.options.directory),
				name: this.options.directory.split(/[\\/]/).filter(Boolean).at(-1) ?? this.options.directory,
				worktree: this.options.directory,
				path: { root: this.options.directory },
				time: { created: Date.now(), updated: Date.now() },
				sandboxes: [],
			})
		}
		return [...byDirectory.values()]
	}

	private async listSessions(params?: { limit?: number; roots?: boolean; search?: string }): Promise<Session[]> {
		await this.ensureInitialized()
		const sessions: Session[] = []
		let cursor: string | undefined
		do {
			const result = (await this.request("session/list", {
				cwd: this.options.directory,
				...(cursor ? { cursor } : {}),
			})) as AcpListSessionsResult
			sessions.push(...(result.sessions ?? []).map((info) => this.rememberSession(info)))
			cursor = result.nextCursor ?? undefined
			if (params?.limit && !params.search && sessions.length >= params.limit) break
			if (params?.limit && params.search) {
				const matching = sessions.filter((session) =>
					(session.title ?? session.id).toLowerCase().includes(params.search!.toLowerCase()),
				)
				if (matching.length >= params.limit) break
			}
		} while (cursor)
		const filtered = params?.search
			? sessions.filter((session) =>
					(session.title ?? session.id).toLowerCase().includes(params.search!.toLowerCase()),
				)
			: sessions
		return filtered.slice(0, params?.limit ?? filtered.length)
	}

	private async createSession(): Promise<Session> {
		await this.ensureInitialized()
		const cwd = this.options.directory ?? defaultCwd()
		const result = (await this.request("session/new", {
			cwd,
			additionalDirectories: [],
			mcpServers: [],
		})) as AcpNewSessionResult
		const session = this.rememberSession({ sessionId: result.sessionId, cwd })
		this.rememberConfigOptions(session.id, cwd, result.configOptions)
		this.emit(session.directory ?? cwd, {
			type: "session.created",
			properties: { info: session, session },
		})
		return session
		}
	private async sessionMessages(sessionId: string, limit?: number): Promise<Array<{ info: Message; parts: Part[] }>> {
		await this.loadSession(sessionId, limit)
		const messages = recentMessages(this.messages.get(sessionId) ?? [], limit)
		return messages.map((info) => ({
			info,
			parts: this.parts.get(partCacheKey(sessionId, info.id)) ?? [],
		}))
	}
		private async loadSession(sessionId: string, limit?: number): Promise<void> {
		const loadedLimit = this.loadedSessionLimits.get(sessionId)
		if (loadedLimitCovers(loadedLimit, limit)) return
		await this.ensureInitialized()
		const session = await this.getSessionById(sessionId)
		const cwd = session?.directory ?? this.sessionDirectories.get(sessionId)
		if (!cwd) throw new Error(`session ${sessionId} not found`)
		const result = (await this.request("session/load", {
			sessionId,
			cwd,
			additionalDirectories: [],
			mcpServers: [],
		})) as AcpLoadSessionResult
		// Electron forwards replay notifications over a separate IPC event channel.
		// Let that channel drain before callers snapshot this client's message store.
		await new Promise((resolve) => setTimeout(resolve, 0))
		this.rememberConfigOptions(sessionId, cwd, result.configOptions)
		// ACP session/load must replay the complete conversation. Apply any
		// caller-requested window locally after the replay has been received.
		this.loadedSessionLimits.set(sessionId, null)
	}

	private async getSessionById(sessionId: string): Promise<Session | undefined> {
		const session = this.sessions.get(sessionId)
		if (session) return session
		return this.discoverSession(sessionId)
	}

	private async discoverSession(sessionId: string): Promise<Session | undefined> {
		const pending = this.sessionDiscovery.get(sessionId)
		if (pending) return pending
		const discovery = this.listSessions()
			.then((sessions) => sessions.find((session) => session.id === sessionId))
			.finally(() => {
				this.sessionDiscovery.delete(sessionId)
			})
		this.sessionDiscovery.set(sessionId, discovery)
		return discovery
	}

	private rememberSession(info: AcpSessionInfo): Session {
		const existing = this.sessions.get(info.sessionId)
		const meta = sessionMeta(info._meta)
		const metadataStatus = sessionStatusFromMetadata(info._meta)
		const parsedCreated = parseTimestampMs(meta?.created_at ?? info.updatedAt)
		const created = parsedCreated ?? existing?.time.created ?? Date.now()
		const parsedUpdated = parseTimestampMs(meta?.updated_at ?? info.updatedAt)
		const updated = parsedUpdated ?? existing?.time.updated ?? created
		const parsedLastActivity = parseTimestampMs(
			meta?.last_activity_at ?? (meta ? undefined : info.updatedAt),
		)
		const lastActivity = parsedLastActivity ?? existing?.time.lastActivity ?? created
		const session: Session = {
			id: info.sessionId,
			title: info.title ?? existing?.title ?? "New session",
			parentID: meta?.parent_session_id ?? existing?.parentID ?? undefined,
			time: { created, updated, lastActivity },
			directory: info.cwd,
			totalInputTokens: meta?.total_input_tokens ?? existing?.totalInputTokens ?? 0,
			totalOutputTokens: meta?.total_output_tokens ?? existing?.totalOutputTokens ?? 0,
			totalTokens: meta?.total_tokens ?? existing?.totalTokens ?? 0,
			totalCacheCreationTokens:
				meta?.total_cache_creation_tokens ?? existing?.totalCacheCreationTokens ?? 0,
			totalCacheReadTokens: meta?.total_cache_read_tokens ?? existing?.totalCacheReadTokens ?? 0,
			promptTokenEstimate: meta?.prompt_token_estimate ?? existing?.promptTokenEstimate ?? 0,
			lastQueryTotalTokens: meta?.last_query_total_tokens ?? existing?.lastQueryTotalTokens ?? 0,
		}
		this.sessions.set(session.id, session)
		this.sessionDirectories.set(session.id, info.cwd)
		this.sessionStatuses.set(
			session.id,
			metadataStatus === undefined
				? this.sessionStatuses.get(session.id) ?? statusFromDevo()
				: statusFromDevo(metadataStatus),
		)
		return session
	}

	private async ensureInitialized(): Promise<void> {
		if (this.initialized) return
		await this.open()
		if (!this.transport) throw new Error("Devo ACP transport is not connected")
		if (this.initialized) return

		let promise = initializePromises.get(this.transport)
		if (!promise) {
			promise = this.request("initialize", DESKTOP_INITIALIZE_PARAMS).then(() => {})
			initializePromises.set(this.transport, promise)
		}
		await promise
		this.initialized = true
	}

	private async open(): Promise<void> {
		if (this.transport) return
		if (this.openPromise) return this.openPromise
		this.openPromise = Promise.resolve()
			.then(() => {
				this.transport = this.options.transport ?? createIpcTransport()
				this.transport.subscribe((event) => this.handleTransportEvent(event))
			})
			.finally(() => {
				this.openPromise = null
			})
		return this.openPromise
	}

	private async request(method: string, params: unknown): Promise<unknown> {
		await this.open()
		if (!this.transport) throw new Error("Devo ACP transport is not connected")
		const validParams = assertValidProtocolPayload({
			method,
			direction: "outgoingRequest",
			payload: params,
		})
		const result = await this.transport.request(method, validParams, this.options.directory)
		return assertValidProtocolPayload({
			method,
			direction: "incomingResult",
			payload: result,
		})
	}

	/**
	 * Canonical protocol RPCs (L2-DES-APP-008): served on every connection
	 * surface via the server's dual-shape dispatch, so the desktop client can
	 * migrate call-by-call while it stays on the ACP surface. Not yet in the
	 * generated schema bundle — the server validates the wire shape.
	 */
	private async requestCanonical(method: string, params: unknown): Promise<unknown> {
		await this.open()
		if (!this.transport) throw new Error("Devo ACP transport is not connected")
		return this.transport.request(method, params, this.options.directory)
	}

	private ensureReferenceSearchSession(): ReferenceSearchSession {
		if (!this.referenceSearchSession) {
			const cwd = this.options.directory ?? defaultCwd()
			this.referenceSearchSession = new ReferenceSearchSession(
				(method, params) => this.requestCanonical(method, params),
				cwd,
			)
		}
		return this.referenceSearchSession
	}

	private handleTransportEvent(event: DevoAcpTransportEvent): void {
		if (event.type === "closed") {
			if (this.transport) {
				initializePromises.delete(this.transport)
			}
			this.initialized = false
			this.events.close()
			return
		}
		if (event.type === "notification" && event.method === "session/update" && event.params) {
			const notification = this.validateTransportPayload<AcpSessionNotification>(
				event.method,
				"incomingNotification",
				event.params,
			)
			if (!notification) return
			this.handleSessionUpdate(notification)
			return
		}
		if (
			event.type === "notification" &&
			event.method &&
			event.params &&
			this.referenceSearchSession?.handleNotification(event.method, event.params)
		) {
			return
		}
		if (
			event.type === "notification" &&
			event.method === "workspace/changes/updated" &&
			event.params
		) {
			const payload = this.validateTransportPayload<WorkspaceChangesUpdatedPayload>(
				event.method,
				"incomingNotification",
				event.params,
			)
			if (!payload) return
			this.handleWorkspaceChangesUpdated(payload)
			return
		}
		if (event.type === "request" && event.id !== undefined && event.method) {
			const params = this.validateTransportPayload(event.method, "incomingRequest", event.params)
			if (!params) return
			this.handleServerRequest(event.id, event.method, params)
		}
	}

	private validateTransportPayload<T>(
		method: string,
		direction:
			| "incomingNotification"
			| "incomingRequest",
		payload: unknown,
	): T | null {
		try {
			return assertValidProtocolPayload<T>({ method, direction, payload })
		} catch (error) {
			this.emitProtocolValidationError(method, payload, error)
			return null
		}
	}

	private handleServerRequest(id: JsonRpcId, method: string, params: unknown): void {
		if (method !== "session/request_permission") return
		const value = params as AcpRequestPermissionParams
		const sessionId = String(value.sessionId ?? "")
		if (!sessionId) return
		const permissionId = `acp-permission-${String(id)}`
		const options = Array.isArray(value.options)
			? value.options.map((option) => ({
					optionId: String(option.optionId),
					kind: String(option.kind),
				}))
			: []
		this.pendingPermissions.set(permissionId, { id, sessionId, options })

		const toolCall = (value.toolCall ?? {}) as Record<string, unknown>
		const permission = String(toolCall.title ?? "Agent requested permission")
		const rawInput = toolCall.rawInput
		const command =
			rawInput && typeof rawInput === "object" && "command" in rawInput
				? String((rawInput as { command: unknown }).command)
				: undefined
		const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
		this.emit(directory, {
			type: "permission.asked",
			properties: {
				id: permissionId,
				requestID: permissionId,
				sessionID: sessionId,
				permission,
				metadata: {
					tool: toolCall.kind,
					command,
				},
			},
		})
	}

	private async respondToPermission(
		permissionId: string,
		response: "once" | "always" | "reject",
	): Promise<void> {
		await this.open()
		if (!this.transport) throw new Error("Devo ACP transport is not connected")
		const pending = this.pendingPermissions.get(permissionId)
		if (!pending) return
		this.pendingPermissions.delete(permissionId)
		const result = {
			outcome: {
				outcome: "selected",
				optionId: permissionOptionId(pending.options, response),
			},
		}
		await this.transport.respond(
			pending.id,
			assertValidProtocolPayload({
				method: "session/request_permission",
				direction: "outgoingResponse",
				payload: result,
			}),
		)
		this.emit(
			this.sessionDirectories.get(pending.sessionId) ?? this.options.directory ?? defaultCwd(),
			{
				type: "permission.replied",
				properties: {
					sessionID: pending.sessionId,
					requestID: permissionId,
				},
			},
		)
	}

	private async respondToQuestion(
		requestId: string,
		answers: QuestionAnswer[],
		eventType: "question.replied" | "question.rejected",
	): Promise<void> {
		const pending = this.pendingQuestions.get(requestId)
		if (!pending) return
		const responseAnswers: Record<string, { answers: string[] }> = {}
		pending.questions.forEach((question, index) => {
			const rawAnswer = answers[index]
			const answerValues = Array.isArray(rawAnswer)
				? rawAnswer.map(String)
				: rawAnswer === undefined || rawAnswer === null
					? []
					: [String(rawAnswer)]
			responseAnswers[question.id] = { answers: answerValues }
		})
		if (pending.id === undefined) {
			throw new Error("pending user-input request has no JSON-RPC request id")
		}
		await this.open()
		if (!this.transport) throw new Error("Devo ACP transport is not connected")
		await this.transport.respond(pending.id, {
			requestId,
			answers: responseAnswers,
		})
		this.pendingQuestions.delete(requestId)
		this.emit(this.sessionDirectories.get(pending.sessionId) ?? this.options.directory ?? defaultCwd(), {
			type: eventType,
			properties: {
				sessionID: pending.sessionId,
				requestID: requestId,
			},
		})
	}

	private handleSessionUpdate(notification: AcpSessionNotification): void {
		const sessionId = notification.sessionId
		const deletedSessionIds = deletedSessionIdsFromOriginalEvent(
			notification._meta?.["devo/originalEvent"],
		)
		if (deletedSessionIds.length > 0) {
			this.handleDeletedSessionIds(
				deletedSessionIds,
				this.sessionDirectories.get(sessionId) ??
					this.sessions.get(sessionId)?.directory ??
					this.options.directory ??
					defaultCwd(),
			)
			return
		}
		const update = notification.update as Record<string, unknown>
		const kind = typeof update.sessionUpdate === "string" ? update.sessionUpdate : undefined
		let session = this.sessions.get(sessionId)
		let directory = this.sessionDirectories.get(sessionId) ?? session?.directory
		if (!session || !directory) {
			const canApplyWithoutDiscoveredSession =
				kind === "user_message_chunk" ||
				kind === "userMessageChunk" ||
				kind === "agent_message_chunk" ||
				kind === "agentMessageChunk" ||
				kind === "agent_thought_chunk" ||
				kind === "agentThoughtChunk" ||
				kind === "tool_call" ||
				kind === "tool_call_update" ||
				kind === "toolCall" ||
				kind === "toolCallUpdate" ||
				kind?.includes("tool") ||
				Boolean(update.toolCallId)
			void this.discoverSession(sessionId)
				.then((discovered) => {
					if (discovered) {
						this.handleSessionUpdate(notification)
						return
					}
					if (!canApplyWithoutDiscoveredSession) return
					const fallbackDirectory = this.options.directory ?? defaultCwd()
					this.rememberSession({ sessionId, cwd: fallbackDirectory })
					this.handleSessionUpdate(notification)
				})
				.catch((error) => {
					if (canApplyWithoutDiscoveredSession) {
						const fallbackDirectory = this.options.directory ?? defaultCwd()
						this.rememberSession({ sessionId, cwd: fallbackDirectory })
						this.handleSessionUpdate(notification)
					} else {
						this.emit(this.options.directory ?? defaultCwd(), sessionErrorEvent(sessionId, error))
					}
				})
			return
		}
		if (kind === "session_info_update" || kind === "sessionInfoUpdate") {
			if (typeof update.title === "string") session.title = update.title
			const meta = sessionMeta(update._meta)
			const metadataUpdated = parseTimestampMs(meta?.updated_at ?? update.updatedAt)
			if (metadataUpdated !== undefined) session.time.updated = metadataUpdated

			const activity = parseTimestampMs(meta?.last_activity_at)
			if (activity !== undefined) session.time.lastActivity = activity

			const metadataStatus = sessionStatusFromMetadata(update._meta)
			if (metadataStatus !== undefined) {
				this.rememberSessionStatus(sessionId, directory, metadataStatus)
			}
		}
		const activityAt = parseTimestampMs(updateMeta(update)?.[DEVO_ACTIVITY_AT_META])
		if (activityAt !== undefined) session.time.lastActivity = activityAt
		this.emit(directory, { type: "session.updated", properties: { info: session, session } })
		this.handleOriginalEvent(sessionId, directory, notification)

		switch (kind) {
			case "user_message_chunk":
			case "userMessageChunk":
				this.appendText(sessionId, directory, "user", "text", update)
				break
			case "agent_message_chunk":
			case "agentMessageChunk":
				this.appendText(sessionId, directory, "assistant", "text", update)
				break
			case "agent_thought_chunk":
			case "agentThoughtChunk":
				this.applyHistoryTurnDuration(sessionId, directory, update)
				this.appendText(sessionId, directory, "assistant", "reasoning", update)
				break
			case "plan":
				this.emitPlan(sessionId, directory, update)
				break
			case "config_option_update":
			case "configOptionUpdate":
				if (Array.isArray(update.configOptions) && update.configOptions.length > 0) {
					this.rememberConfigOptions(sessionId, directory, update.configOptions as AcpConfigOption[])
				}
				this.emit(directory, {
					type: "session.config.updated",
					properties: { sessionID: sessionId, configOptions: update.configOptions ?? [] },
				})
				break
			case "available_commands_update":
			case "availableCommandsUpdate":
				this.emit(directory, {
					type: "session.commands.updated",
					properties: { sessionID: sessionId, commands: update.availableCommands ?? [] },
				})
				break
			case "current_mode_update":
			case "currentModeUpdate":
				this.emit(directory, {
					type: "session.mode.updated",
					properties: { sessionID: sessionId, modeID: update.currentModeId },
				})
				break
			case "usage_update":
			case "usageUpdate":
				this.emit(directory, {
					type: "session.usage.updated",
					properties: {
						sessionID: sessionId,
						used: update.used,
						size: update.size,
						cost: update.cost,
					},
				})
				break
			case "tool_call":
			case "tool_call_update":
			case "toolCall":
			case "toolCallUpdate":
				this.appendTool(sessionId, directory, update)
				break
			default:
				if (kind?.includes("tool") || update.toolCallId) {
					this.appendTool(sessionId, directory, update)
				}
		}
	}

	private handleOriginalEvent(
		sessionId: string,
		directory: string,
		notification: AcpSessionNotification,
	): void {
		const original = notification._meta?.["devo/originalEvent"]
		if (!original || typeof original !== "object") return
		const originalMethod =
			typeof notification._meta?.["devo/originalMethod"] === "string"
				? notification._meta["devo/originalMethod"]
				: undefined
		const deletedSessionIds = deletedSessionIdsFromOriginalEvent(original)
		if (deletedSessionIds.length > 0) {
			this.handleDeletedSessionIds(deletedSessionIds, directory)
			return
		}
		const retryStatus = providerRetryStatusFromOriginalEvent(original as Record<string, unknown>, originalMethod)
		if (retryStatus) {
			this.emit(directory, {
				type: "turn.provider_retry_status",
				properties: retryStatus,
			})
			return
		}
		const turnFailure = turnFailureFromOriginalEvent(original as Record<string, unknown>, originalMethod)
		if (turnFailure) {
			this.emit(directory, {
				type: "session.error",
				properties: {
					sessionID: turnFailure.sessionID,
					error: {
						name: turnFailure.code,
						data: { message: turnFailure.message },
					},
				},
			})
			return
		}
		const changedStatus = sessionStatusChangedFromOriginalEvent(original, originalMethod)
		if (changedStatus) {
			this.rememberSessionStatus(changedStatus.sessionId, directory, changedStatus.status)
			return
		}
		const compaction = sessionCompactionFromOriginalEvent(original, originalMethod)
		if (compaction) {
			this.emit(directory, {
				type: `session.compaction.${compaction.status}`,
				properties: {
					sessionID: compaction.sessionId,
					...(compaction.message ? { message: compaction.message } : {}),
				},
			})
			if (compaction.status === "started" && compaction.itemId) {
				const messageId = `compaction-${compaction.itemId}`
				const markerExists = this.parts
					.get(partCacheKey(sessionId, messageId))
					?.some((part) => part.type === "text" && part.text === "Session compaction started.")
				if (!markerExists) {
					this.appendText(sessionId, directory, "assistant", "text", {
						content: { text: "Session compaction started." },
						messageId,
						...(compaction.turnId
							? { _meta: { [DEVO_TURN_ID_META]: compaction.turnId } }
							: {}),
					})
				}
			}
			return
		}
		const payload = requestUserInputFromOriginalEvent(original)
		if (payload) {
			this.handleRequestUserInput(sessionId, directory, payload)
		}
		const workspaceChanges = workspaceChangesUpdatedFromOriginalEvent(original)
		if (workspaceChanges) {
			this.handleWorkspaceChangesUpdated(workspaceChanges, directory)
		}
		if ("ServerRequestResolved" in original) {
			const payload = (original as { ServerRequestResolved: Record<string, unknown> })
				.ServerRequestResolved
			const requestId = String(payload.request_id ?? payload.requestId ?? "")
			const pending = this.pendingQuestions.get(requestId)
			if (!pending) return
			this.pendingQuestions.delete(requestId)
			this.emit(directory, {
				type: "question.replied",
				properties: { sessionID: pending.sessionId, requestID: requestId },
			})
		}
	}

	private rememberSessionStatus(sessionId: string, directory: string, protocolStatus: string): void {
		const status = statusFromDevo(protocolStatus)
		this.sessionStatuses.set(sessionId, status)
		this.emit(directory, {
			type: "session.status",
			properties: { sessionID: sessionId, status },
		})
	}

	private handleDeletedSessionIds(sessionIds: string[], fallbackDirectory: string): void {
		for (const sessionId of sessionIds) {
			const { directory, known } = this.forgetSession(sessionId, fallbackDirectory)
			if (known) this.emitSessionDeleted(sessionId, directory)
		}
	}

	private forgetSession(
		sessionId: string,
		fallbackDirectory = this.options.directory ?? defaultCwd(),
	): { directory: string; known: boolean } {
		const session = this.sessions.get(sessionId)
		const directory = this.sessionDirectories.get(sessionId) ?? session?.directory ?? fallbackDirectory
		const known =
			this.sessions.has(sessionId) ||
			this.sessionStatuses.has(sessionId) ||
			this.sessionDirectories.has(sessionId) ||
			this.loadedSessionLimits.has(sessionId) ||
			this.messages.has(sessionId)
		this.sessions.delete(sessionId)
		this.sessionStatuses.delete(sessionId)
		this.sessionDirectories.delete(sessionId)
		this.loadedSessionLimits.delete(sessionId)
		this.messages.delete(sessionId)
		for (const [messageId, parts] of this.parts) {
			if (parts.some((part) => part.sessionID === sessionId)) {
				this.parts.delete(messageId)
			}
		}
		return { directory, known }
	}

	private emitSessionDeleted(sessionId: string, directory: string): void {
		this.emit(directory, {
			type: "session.deleted",
			properties: { info: { id: sessionId, directory } },
		})
	}

	private handleWorkspaceChangesUpdated(
		payload: WorkspaceChangesUpdatedPayload,
		directory?: string,
	): void {
		const event = workspaceChangesUpdatedEventProperties(payload)
		if (!event.sessionID) return
		const emitDirectory =
			directory ?? this.sessionDirectories.get(event.sessionID) ?? this.options.directory ?? defaultCwd()
		this.emit(emitDirectory, {
			type: "workspace.changes.updated",
			properties: event,
		})
	}

	private handleRequestUserInput(
		sessionId: string,
		directory: string,
		payload: Record<string, unknown>,
	): void {
		const request = (payload.request ?? {}) as Record<string, unknown>
		const requestId = String(request.request_id ?? request.requestId ?? "")
		if (!requestId) return
		const requestSessionId = String(request.session_id ?? request.sessionId ?? sessionId)
		const rawQuestions = Array.isArray(payload.questions) ? payload.questions : []
		const questions = rawQuestions.map(questionInfoFromAcp)
		const existing = this.pendingQuestions.get(requestId)
		this.pendingQuestions.set(requestId, {
			id: existing?.id,
			sessionId: requestSessionId,
			questions,
		})
		this.emit(directory, {
			type: "question.asked",
			properties: {
				id: requestId,
				requestID: requestId,
				sessionID: requestSessionId,
				questions,
			},
		})
	}

	private turnKey(sessionId: string, turnId: string): string {
		return `${sessionId}\u001f${turnId}`
	}

	private turnIdForUpdate(update: Record<string, unknown>): string | undefined {
		return updateMetaString(update, DEVO_TURN_ID_META)
	}

	private parentMessageIdForUpdate(
		sessionId: string,
		update: Record<string, unknown>,
		existingMessage?: Message,
	): string | undefined {
		const turnId = this.turnIdForUpdate(update)
		return (
			updateMetaString(update, DEVO_PARENT_MESSAGE_ID_META) ??
			(turnId ? this.userMessageByTurn.get(this.turnKey(sessionId, turnId)) : undefined) ??
			existingMessage?.parentID ??
			this.lastUserMessageBySession.get(sessionId)
		)
	}

	private earliestMessageCreatedForTurn(sessionId: string, turnId: string): number | undefined {
		let earliest: number | undefined
		for (const message of this.messages.get(sessionId) ?? []) {
			if (this.messageTurnIds.get(partCacheKey(sessionId, message.id)) !== turnId) continue
			const created = message.time?.created
			if (typeof created !== "number" || !Number.isFinite(created)) continue
			earliest = earliest === undefined ? created : Math.min(earliest, created)
		}
		return earliest
	}

	private messageCreatedAtForUpdate(
		sessionId: string,
		role: "assistant" | "user",
		messageId: string,
		update: Record<string, unknown>,
		existingMessage: Message | undefined,
		now: number,
	): number {
		let created =
			existingMessage?.time?.created ?? updateHistoryCreatedAt(update) ?? historyMessageCreatedAt(messageId) ?? now
		const turnId = this.turnIdForUpdate(update)
		if (role === "user" && turnId) {
			const earliest = this.earliestMessageCreatedForTurn(sessionId, turnId)
			if (earliest !== undefined && earliest <= created) {
				created = Math.max(0, earliest - 1)
			}
		}
		return created
	}

	private rememberMessageTurn(
		sessionId: string,
		directory: string,
		messageId: string,
		role: "assistant" | "user",
		update: Record<string, unknown>,
	): void {
		const turnId = this.turnIdForUpdate(update)
		if (!turnId) return
		this.messageTurnIds.set(partCacheKey(sessionId, messageId), turnId)
		if (role !== "user") return
		this.userMessageByTurn.set(this.turnKey(sessionId, turnId), messageId)
		this.reparentTurnMessages(sessionId, directory, turnId, messageId)
	}

	private reparentTurnMessages(
		sessionId: string,
		directory: string,
		turnId: string,
		userMessageId: string,
	): void {
		const messages = this.messages.get(sessionId)
		if (!messages) return
		for (let index = 0; index < messages.length; index++) {
			const message = messages[index]
			if (message.role !== "assistant") continue
			if (this.messageTurnIds.get(partCacheKey(sessionId, message.id)) !== turnId) continue
			if (message.parentID === userMessageId) continue
			const updated = { ...message, parentID: userMessageId } as Message
			messages[index] = updated
			this.emit(directory, { type: "message.updated", properties: { info: updated, message: updated } })
		}
	}

	private applyHistoryTurnDuration(
		sessionId: string,
		directory: string,
		update: Record<string, unknown>,
	): void {
		const durationMs = Math.floor(
			numberFromProtocol(updateMeta(update)?.[DEVO_TURN_DURATION_MS_META]),
		)
		if (durationMs <= 0) return
		const parentID =
			updateMetaString(update, DEVO_PARENT_MESSAGE_ID_META) ??
			this.lastUserMessageBySession.get(sessionId)
		if (!parentID) return
		const messages = this.messages.get(sessionId)
		if (!messages) return
		const userMessage = messages.find(
			(message) => message.id === parentID && message.role === "user",
		)
		const userCreated = userMessage?.time?.created
		if (typeof userCreated !== "number" || !Number.isFinite(userCreated)) return

		for (let index = messages.length - 1; index >= 0; index--) {
			const message = messages[index]
			if (message.role !== "assistant" || message.parentID !== parentID) continue
			if (typeof message.time?.completed === "number") return
			const updated = {
				...message,
				time: { ...(message.time ?? {}), completed: userCreated + durationMs },
			} as Message
			messages[index] = updated
			this.emit(directory, { type: "message.updated", properties: { info: updated, message: updated } })
			return
		}
	}

	private appendText(
		sessionId: string,
		directory: string,
		role: "assistant" | "user",
		partType: "reasoning" | "text",
		update: Record<string, unknown>,
	): void {
		const text = textFromUpdate(update)
		if (!text) return
		const now = this.nextEventTime()
		const messageId =
			typeof update.messageId === "string"
				? update.messageId
				: `${role}-${sessionId}-${now}`
		const existingMessage = this.messages.get(sessionId)?.find((message) => message.id === messageId)
		const parentID =
			role === "assistant" ? this.parentMessageIdForUpdate(sessionId, update, existingMessage) : undefined
		const created = this.messageCreatedAtForUpdate(
			sessionId,
			role,
			messageId,
			update,
			existingMessage,
			now,
		)
		const turnId = this.turnIdForUpdate(update)
		const message = {
			...(existingMessage ?? {}),
			id: messageId,
			sessionID: sessionId,
			role,
			...(parentID ? { parentID } : {}),
			...(turnId ? { turnID: turnId } : {}),
			time: { ...(existingMessage?.time ?? {}), created },
		} as Message
		this.appendMessage(sessionId, message)
		if (role === "user") this.lastUserMessageBySession.set(sessionId, messageId)
		this.emit(directory, { type: "message.updated", properties: { info: message, message } })
		this.rememberMessageTurn(sessionId, directory, messageId, role, update)

		const partId = `${messageId}-${partType === "reasoning" ? "reasoning" : "text"}`
		const existingPart = this.parts
			.get(partCacheKey(sessionId, messageId))
			?.find((part) => part.id === partId)
		const field = partType === "reasoning" ? "text" : "text"
		const existingText =
			messageId.startsWith("history-") || typeof existingPart?.[field] !== "string"
				? ""
				: existingPart[field]
		const partEventTime = updateHistoryCreatedAt(update) ?? now
		const metadata = textPartMetadataFromUpdate(update, existingPart)
		const part = {
			id: partId,
			sessionID: sessionId,
			messageID: messageId,
			type: partType,
			[field]: `${existingText}${text}`,
			...(metadata ? { metadata } : {}),
			time: partTime(existingPart, partEventTime),
		} as TextPart | ReasoningPart
		this.appendPart(sessionId, messageId, part)
		this.emit(directory, { type: "message.part.updated", properties: { part } })
	}

	private emitPlan(sessionId: string, directory: string, update: Record<string, unknown>): void {
		const entries = Array.isArray(update.entries) ? update.entries : []
		const todos = entries.map((entry) => {
			const value = entry as Record<string, unknown>
			return {
				content: String(value.content ?? value.title ?? ""),
				status: String(value.status ?? "pending"),
			}
		})
		this.emit(directory, {
			type: "todo.updated",
			properties: { sessionID: sessionId, todos },
		})
	}

	private appendTool(sessionId: string, directory: string, update: Record<string, unknown>): void {
		const now = this.nextEventTime()
		const toolCallId = toolCallIdFromUpdate(update, now)
		const messageId = `tool-${toolCallId}`
		const existingMessage = this.messages.get(sessionId)?.find((message) => message.id === messageId)
		const existingPart = this.parts
			.get(partCacheKey(sessionId, messageId))
			?.find((part) => part.id === `${messageId}-part`)
		const parentID = this.parentMessageIdForUpdate(sessionId, update, existingMessage)
		const created = this.messageCreatedAtForUpdate(
			sessionId,
			"assistant",
			messageId,
			update,
			existingMessage,
			now,
		)
		const turnId = this.turnIdForUpdate(update)
		const message = {
			...(existingMessage ?? {}),
			id: messageId,
			sessionID: sessionId,
			role: "assistant",
			...(parentID ? { parentID } : {}),
			...(turnId ? { turnID: turnId } : {}),
			time: { ...(existingMessage?.time ?? {}), created },
		} as Message
		const partEventTime = updateHistoryCreatedAt(update) ?? now
		const part = toolPartFromUpdate(sessionId, update, existingPart, partEventTime) as ToolPart
		this.appendMessage(sessionId, message)
		this.rememberMessageTurn(sessionId, directory, messageId, "assistant", update)
		this.appendPart(sessionId, message.id, part)
		this.emit(directory, { type: "message.updated", properties: { info: message, message } })
		this.emit(directory, { type: "message.part.updated", properties: { part } })
	}

	private completeOpenAssistantMessages(
		sessionId: string,
		directory: string,
		promptStartedAt: number,
	): void {
		const messages = this.messages.get(sessionId)
		if (!messages) return
		let completedAt: number | null = null
		for (let index = 0; index < messages.length; index++) {
			const message = messages[index]
			if (message.role !== "assistant" || message.time.completed != null) continue
			if (message.time.created < promptStartedAt) continue
			completedAt ??= this.nextEventTime()
			const updated = {
				...message,
				time: { ...message.time, completed: completedAt },
			} as Message
			messages[index] = updated
			this.emit(directory, { type: "message.updated", properties: { info: updated, message: updated } })
		}
	}

	private appendMessage(sessionId: string, message: Message): void {
		const messages = this.messages.get(sessionId) ?? []
		const index = messages.findIndex((existing) => existing.id === message.id)
		if (index >= 0) {
			messages[index] = message
		} else {
			messages.push(message)
		}
		this.messages.set(sessionId, messages)
	}

	private appendPart(sessionId: string, messageId: string, part: Part): void {
		const key = partCacheKey(sessionId, messageId)
		const parts = this.parts.get(key) ?? []
		const index = parts.findIndex((existing) => existing.id === part.id)
		if (index >= 0) {
			parts[index] = part
		} else {
			parts.push(part)
		}
		this.parts.set(key, parts)
	}

	private nextEventTime(): number {
		const now = Date.now()
		const eventTime = Math.max(now, this.lastEventTime + 1)
		this.lastEventTime = eventTime
		return eventTime
	}

	private rememberConfigOptions(
		sessionId: string,
		directory: string,
		configOptions?: AcpConfigOption[],
	): void {
		if (!Array.isArray(configOptions)) return
		this.configOptionsBySession.set(sessionId, configOptions)
		this.rememberDirectoryConfigOptions(directory, configOptions)
	}

	private rememberDirectoryConfigOptions(
		directory: string,
		configOptions?: AcpConfigOption[] | null,
	): void {
		if (!Array.isArray(configOptions)) return
		this.configOptionsByDirectory.set(directory, configOptions)
	}

	private async setSessionConfigOption(
		sessionId: string,
		configId: string,
		value: string,
	): Promise<void> {
		const setConfigParams: AcpSetConfigOptionParams = {
			sessionId,
			configId,
			value,
		}
		const result = (await this.request("session/set_config_option", setConfigParams)) as AcpSetConfigOptionResult
		const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
		this.rememberConfigOptions(sessionId, directory, result.configOptions)
	}

	private async setDefaultConfigOption(
		configId: string,
		value: string,
	): Promise<AcpConfigOption[]> {
		await this.ensureInitialized()
		const directory = this.options.directory ?? defaultCwd()
		// Canonical model/preferences/write (ratified #12): configId maps to
		// the patch fields; the result converts back to the select shape the
		// config UI renders.
		const patch: Record<string, string> = {}
		if (configId === "model") patch.model = value
		else if (configId === "thought_level") patch.reasoningEffort = value
		else throw new Error(`unknown model config option '${configId}'`)
		const params: Record<string, unknown> = { patch }
		if (this.options.directory) params.cwd = this.options.directory
		const result = (await this.requestCanonical("model/preferences/write", params)) as {
			preferences?: ModelPreferencesWire
		}
		const options = acpConfigOptionsFromModelPreferences(result.preferences ?? {})
		this.rememberDirectoryConfigOptions(directory, options)
		return this.currentConfigOptions()
	}

	private cachedConfigOptions(): AcpConfigOption[] | undefined {
		if (this.options.directory) {
			const byDirectory = this.configOptionsByDirectory.get(this.options.directory)
			if (byDirectory) return byDirectory
		}
		return this.configOptionsBySession.values().next().value
	}

	private currentConfigOptions(): AcpConfigOption[] {
		return this.cachedConfigOptions() ?? []
	}

	invalidateConfigOptionCaches(): void {
		this.configOptionsBySession.clear()
		this.configOptionsByDirectory.clear()
	}

	private async ensureCurrentConfigOptions(): Promise<AcpConfigOption[]> {
		const cached = this.cachedConfigOptions()
		if (cached) return cached

		await this.ensureInitialized()
		const directory = this.options.directory ?? defaultCwd()
		// Canonical model/preferences/read (ratified #12).
		const params: Record<string, unknown> = this.options.directory ? { cwd: this.options.directory } : {}
		const result = (await this.requestCanonical("model/preferences/read", params)) as {
			preferences?: ModelPreferencesWire
		}
		this.rememberDirectoryConfigOptions(directory, acpConfigOptionsFromModelPreferences(result.preferences ?? {}))
		return this.currentConfigOptions()
	}

	private emit(directory: string, payload: Event): void {
		this.events.push({ directory, payload })
	}

	private emitProtocolValidationError(method: string, payload: unknown, error: unknown): void {
		const sessionId = sessionIdFromPayload(payload) ?? "protocol"
		const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
		const reason =
			error instanceof ProtocolValidationError
				? error
				: new ProtocolValidationError({
						method,
						direction: "incomingNotification",
						payload,
						message: error instanceof Error ? error.message : String(error),
					})
		this.emit(directory, sessionErrorEvent(sessionId, reason))
	}
}

export type DevoClient = any

export function createDevoClient(options: CreateDevoClientOptions = {}): DevoClient {
	return new AcpClient(options)
}

function sessionIdFromPayload(payload: unknown): string | null {
	if (!payload || typeof payload !== "object") return null
	const value = payload as Record<string, unknown>
	for (const key of ["sessionId", "session_id"]) {
		if (typeof value[key] === "string") return value[key] as string
	}
	return null
}
