// @ts-nocheck

import {
	AsyncEventQueue,
	type SessionConfigOption,
	configDataFromConfigOptions,
	createIpcTransport,
	defaultCwd,
	permissionOptionId,
	partTime,
	providerDataFromConfigOptions,
	questionInfoFromNative,
	requestUserInputFromOriginalEvent,
	sessionErrorEvent,
	stableId,
	statusFromDevo,
	textFromUpdate,
	toolCallIdFromUpdate,
	toolPartFromUpdate,
} from "./native-client-support"
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
} from "./generated/native"
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

type LegacySessionInfo = {
	sessionId: string
	cwd: string
	title?: string
	updatedAt?: string
	_meta?: Record<string, unknown>
}
type LegacySessionNotification = {
	sessionId: string
	update: Record<string, unknown>
	_meta?: Record<string, unknown>
}

export interface DevoNativeTransportEvent {
	type: "notification" | "request" | "closed"
	id?: JsonRpcId
	method?: string
	params?: unknown
	error?: string
}

export interface DevoNativeTransport {
	request(method: string, params?: unknown, directory?: string): Promise<unknown>
	notify?(method: string, params?: unknown, directory?: string): Promise<void>
	respond(id: JsonRpcId, result: unknown): Promise<void>
	subscribe(listener: (event: DevoNativeTransportEvent) => void): () => void
	connected(): boolean
}

export interface CreateDevoClientOptions {
	baseUrl?: string
	directory?: string
	fetch?: typeof fetch
	transport?: DevoNativeTransport
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
export type PermissionResponse =
	| "once"
	| "turn"
	| "session"
	| "pathPrefix"
	| "host"
	| "tool"
	| "commandPrefix"
	| "commandPrefixPersist"
	| "always"
	| "reject"
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
} from "./generated/native"

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
	method?: string
	sessionId: string
	questions: QuestionInfo[]
}

type PendingPermission = {
	id: JsonRpcId
	method: string
	sessionId?: string
	options: Array<{ optionId: string; kind: string }>
	availableScopes?: string[]
	native?: boolean
}

function partCacheKey(sessionId: string, messageId: string): string {
	return `${sessionId}\u001f${messageId}`
}

function objectRecord(value: unknown): Record<string, unknown> | undefined {
	return value && typeof value === "object" ? (value as Record<string, unknown>) : undefined
}

function fileChangeInput(item: Record<string, unknown>): Record<string, unknown> | undefined {
	const changes = Array.isArray(item.changes) ? item.changes : []
	const first = objectRecord(changes[0])
	if (!first) return undefined
	const path = typeof first.path === "string" ? first.path : undefined
	const change = objectRecord(first.change)
	const content =
		typeof change?.content === "string"
			? change.content
			: typeof first.content === "string"
				? first.content
				: undefined
	if (!path && content == null) return undefined
	return { filePath: path, path, content }
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

type ContextOccupancyWire = {
	totalTokens: number
	contextWindowTokens: number
	categories: Array<{ id: string; tokens: number; shareBps: number }>
}

function contextOccupancyFromProtocol(value: unknown): ContextOccupancyWire | null {
	const occupancy = objectRecord(value)
	if (!occupancy) return null
	const rawCategories = Array.isArray(occupancy.categories) ? occupancy.categories : []
	return {
		totalTokens: numberFromProtocol(occupancy.totalTokens ?? occupancy.total_tokens),
		contextWindowTokens: numberFromProtocol(
			occupancy.contextWindowTokens ?? occupancy.context_window_tokens,
		),
		categories: rawCategories.flatMap((entry) => {
			const category = objectRecord(entry)
			const id = String(category?.id ?? "")
			if (!id) return []
			return [
				{
					id,
					tokens: numberFromProtocol(category?.tokens),
					shareBps: numberFromProtocol(category?.shareBps ?? category?.share_bps),
				},
			]
		}),
	}
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

/** Canonical `model/preferences` wire shape (ratified #12). */
type PreferencesOptionWire = {
	value: string
	label: string
	description?: string
	/** Present on `availableModels` entries: that model's effort choices. */
	availableEfforts?: PreferencesOptionWire[]
}

type ModelPreferencesWire = {
	model?: string
	reasoningEffort?: string
	availableModels?: PreferencesOptionWire[]
	availableEfforts?: PreferencesOptionWire[]
}

/** Canonical model preferences → the select options the config UI renders. */
function sessionConfigOptionsFromModelPreferences(preferences: ModelPreferencesWire): SessionConfigOption[] {
	const toSelectOptions = (entries?: PreferencesOptionWire[]) =>
		(entries ?? []).map((entry) => ({
			value: entry.value,
			name: entry.label,
			...(entry.description !== undefined ? { description: entry.description } : {}),
			...(entry.availableEfforts?.length
				? {
						availableEfforts: entry.availableEfforts.map((effort) => ({
							value: effort.value,
							name: effort.label,
							...(effort.description !== undefined ? { description: effort.description } : {}),
						})),
					}
				: {}),
		}))
	const options: SessionConfigOption[] = []
	if (preferences.model !== undefined || (preferences.availableModels?.length ?? 0) > 0) {
		options.push({
			type: "select",
			id: "model",
			name: "Model",
			description: "Controls the model used for this session",
			category: "model",
			currentValue: preferences.model ?? "",
			options: toSelectOptions(preferences.availableModels),
		} as SessionConfigOption)
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
		} as SessionConfigOption)
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
	const value = payload as unknown as Record<string, unknown>
	const stats = objectRecord(value.stats) ?? {}
	return {
		sessionID: value.sessionId ?? value.session_id,
		turnID: value.turnId ?? value.turn_id,
		scope: payload.scope,
		status: payload.status,
		coverage: payload.coverage,
		changeSetStatus: value.changeSetStatus ?? value.change_set_status,
		stats: {
			filesChanged: numberFromProtocol(stats.filesChanged ?? stats.files_changed),
			additions: numberFromProtocol(stats.additions),
			deletions: numberFromProtocol(stats.deletions),
		},
		version: numberFromProtocol(payload.version),
		generatedAt: value.generatedAt ?? value.generated_at,
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
const DEVO_COMPACTION_STATUS_META = "devo/compactionStatus"
const COMPACTION_STARTED_LABEL = "Compacting context"
const COMPACTION_COMPLETED_LABEL = "Context compacted"

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
	if (meta?.[DEVO_ITEM_KIND_META] === "context_compaction") {
		metadata[DEVO_ITEM_KIND_META] = "context_compaction"
		const status = meta[DEVO_COMPACTION_STATUS_META]
		if (typeof status === "string" && status) {
			metadata[DEVO_COMPACTION_STATUS_META] = status
		}
	}
	if (meta?.[DEVO_ITEM_KIND_META] === "proposed_plan" || meta?.[DEVO_ITEM_KIND_META] === "plan") {
		metadata[DEVO_ITEM_KIND_META] = meta[DEVO_ITEM_KIND_META]
		if (meta.planEntries !== undefined) metadata.planEntries = meta.planEntries
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

const initializePromises = new WeakMap<DevoNativeTransport, Promise<void>>()

export const DESKTOP_INITIALIZE_PARAMS = {
	protocolVersion: 1,
	_meta: { devo: { protocol: "native", typedItems: true } },
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

class NativeClient {
	private transport: DevoNativeTransport | null = null
	private openPromise: Promise<void> | null = null
	private initialized = false
	private events = new AsyncEventQueue<GlobalEvent>()
	private sessions = new Map<string, Session>()
	private sessionDirectories = new Map<string, string>()
	private sessionStatuses = new Map<string, SessionStatus>()
	private promptStartedAtBySession = new Map<string, number>()
	private messages = new Map<string, Message[]>()
	private parts = new Map<string, Part[]>()
	private loadedSessionLimits = new Map<string, LoadedSessionLimit>()
	private lastUserMessageBySession = new Map<string, string>()
	private userMessageByTurn = new Map<string, string>()
	private messageTurnIds = new Map<string, string>()
	private configOptionsBySession = new Map<string, SessionConfigOption[]>()
	private configOptionsByDirectory = new Map<string, SessionConfigOption[]>()
	private pendingPermissions = new Map<string, PendingPermission>()
	private pendingQuestions = new Map<string, PendingQuestion>()
	private subscriptions = new Map<string, { subscriptionId: string; cursors: Array<{ streamId: string; seq: number }> }>()
	private subscriptionCursors = new Map<string, Array<{ streamId: string; seq: number }>>()
	private renderedNativeItems = new Set<string>()
	private turnSessions = new Map<string, string>()
	private nativeItemCallIds = new Map<string, string>()
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
			const directory = this.sessionDirectories.get(params.sessionID) ?? this.options.directory ?? defaultCwd()
			this.lastUserMessageBySession.delete(params.sessionID)
			const promptStartedAt = Math.max(Date.now(), this.lastEventTime + 1)
			this.promptStartedAtBySession.set(params.sessionID, promptStartedAt)
			const busyStatus = { type: "busy" }
			this.sessionStatuses.set(params.sessionID, busyStatus)
			this.emit(directory, {
				type: "session.status",
				properties: { sessionID: params.sessionID, status: busyStatus },
			})
			// turn/start returns when the turn is accepted, not when it finishes.
			// Stay busy until turn/completed (or a failed start below).
			try {
				await this.turn.start({
					sessionID: params.sessionID,
					parts: params.parts,
					model: params.model,
					variant: params.variant,
					collaborationMode: params.collaborationMode,
				})
			} catch (error) {
				this.promptStartedAtBySession.delete(params.sessionID)
				this.completeOpenAssistantMessages(params.sessionID, directory, promptStartedAt)
				const idleStatus = { type: "idle" }
				this.sessionStatuses.set(params.sessionID, idleStatus)
				this.emit(directory, sessionErrorEvent(params.sessionID, error))
				this.emit(directory, {
					type: "session.status",
					properties: { sessionID: params.sessionID, status: idleStatus },
				})
			}
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
			const metadata = {
				...(result.session ?? {}),
				id: String(result.session?.id ?? params.sessionID),
				title: result.session?.title ?? params.title,
			}
			const session = this.rememberNativeSession(metadata)
			this.emit(session.directory ?? this.options.directory ?? defaultCwd(), {
				type: "session.updated",
				properties: { info: session, session },
			})
			return { data: session }
		},
		delete: async (params: { sessionID: string }) => {
			await this.requestCanonical("session/delete", { sessionId: params.sessionID })
			const { directory } = this.forgetSession(params.sessionID)
			this.emitSessionDeleted(params.sessionID, directory)
		},
		get: async (params: { sessionID: string }) => ({
			data: await this.getSessionById(params.sessionID),
		}),
		diff: async (params: { sessionID: string }) => {
			const result = (await this.requestCanonical("workspace/changes/read", {
				sessionId: params.sessionID,
				scopes: ["uncommitted"],
				diffDetail: "full",
				maxDiffBytes: 2_000_000,
			})) as { views?: Array<Record<string, unknown>> }
			return {
				data: (result.views ?? [])
					.map((view) => view.unifiedDiff)
					.filter((diff): diff is string => typeof diff === "string" && diff.length > 0)
					.map((diff) => ({ diff })),
			}
		},
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
			if (params.collaborationMode) await this.setSessionConfigOption(params.sessionID, "mode", params.collaborationMode)
			await this.ensureSessionSubscription(params.sessionID)
			const result = (await this.requestCanonical("turn/start", {
				sessionId: params.sessionID,
				input: inputItemsFromPromptParts(params.parts),
				idempotencyKey: crypto.randomUUID(),
			})) as { turn: unknown }
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
			response: PermissionResponse
		}) => {
			await this.respondToPermission(params.permissionID, params.response)
		},
		reply: async (params: { requestID: string; reply?: PermissionResponse }) => {
			await this.respondToPermission(params.requestID, params.reply ?? "reject")
		},
	}

	instance = {
		dispose: async () => {},
	}

	global = {
		dispose: async () => {},
		event: async () => {
			await this.ensureKnownSessionSubscriptions()
			return { stream: this.events }
		},
		config: {
			update: async (_params: unknown) => ({ data: null }),
		},
	}

	event = {
		subscribe: async () => {
			await this.ensureKnownSessionSubscriptions()
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
		skills: async () => {
			const result = (await this.requestCanonical("skill/list", {
				...(this.options.directory ? { cwd: this.options.directory } : {}),
				forceReload: false,
			})) as { skills?: unknown[] }
			return { data: result.skills ?? [] }
		},
		setSkillEnabled: async (params: { path: string; enabled: boolean }) => {
			const result = (await this.requestCanonical("skill/set_enabled", {
				path: params.path,
				enabled: params.enabled,
				...(this.options.directory ? { cwd: this.options.directory } : {}),
			})) as { skills?: unknown[] }
			return { data: result.skills ?? [] }
		},
	}

	context = {
		usage: {
			read: async (params: { sessionID: string }) => {
				const result = (await this.requestCanonical("context/usage/read", {
					sessionId: params.sessionID,
				})) as { occupancy?: unknown }
				this.emitContextUsage(params.sessionID, result.occupancy)
				return { data: result.occupancy }
			},
		},
	}

	mcp = {
		list: async () => {
			const result = (await this.requestCanonical("mcp/list", {})) as { servers?: unknown[] }
			return { data: result.servers ?? [] }
		},
		tools: async (params: { name: string }) => {
			const result = (await this.requestCanonical("mcp/tools", { name: params.name })) as {
				tools?: unknown[]
			}
			return { data: result.tools ?? [] }
		},
		setEnabled: async (params: { name: string; enabled: boolean }) => ({
			data: await this.requestCanonical("mcp/set_enabled", params),
		}),
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
			const result = (await this.requestCanonical("session/list", {
				cwds: this.options.directory ? [this.options.directory] : [],
				...(params?.search ? { search: params.search } : {}),
				...(cursor ? { cursor } : {}),
				...(params?.limit ? { limit: params.limit } : {}),
			})) as { data?: Array<Record<string, unknown>>; nextCursor?: string | null }
			sessions.push(...(result.data ?? []).map((info) => this.rememberNativeSession(info)))
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
		const result = (await this.requestCanonical("session/new", {
			cwd,
			idempotencyKey: crypto.randomUUID(),
		})) as { session: Record<string, unknown> }
		const session = this.rememberNativeSession(result.session)
		await this.ensureSessionSubscription(session.id)
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
		const resumed = (await this.requestCanonical("session/resume", {
			sessionId,
		})) as { session: Record<string, unknown>; lastContextOccupancy?: unknown; last_context_occupancy?: unknown }
		this.rememberNativeSession(resumed.session)
		this.emitContextUsage(
			sessionId,
			resumed.lastContextOccupancy ?? resumed.last_context_occupancy,
		)
		await this.ensureSessionSubscription(sessionId)
		let cursor: string | undefined
		do {
			const page = (await this.requestCanonical("session/items/list", {
				sessionId,
				...(cursor ? { cursor } : {}),
				limit: 500,
			})) as { data?: Array<Record<string, unknown>>; nextCursor?: string | null }
			for (const item of page.data ?? []) this.handleNativeItemEnvelope(item, "item/completed")
			cursor = page.nextCursor ?? undefined
		} while (cursor)
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

	private rememberSession(info: LegacySessionInfo): Session {
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

	private rememberNativeSession(info: Record<string, unknown>): Session {
		const id = String(info.id ?? "")
		if (!id) throw new Error("Native session is missing id")
		const existing = this.sessions.get(id)
		const usage = objectRecord(info.usage)
		const total = objectRecord(usage?.total)
		const created = parseTimestampMs(info.createdAt) ?? existing?.time.created ?? Date.now()
		const updated = parseTimestampMs(info.lastActivityAt) ?? existing?.time.updated ?? created
		const parent = objectRecord(info.parent)
		const session: Session = {
			id,
			title: typeof info.title === "string" ? info.title : existing?.title ?? "New session",
			parentID: typeof parent?.sessionId === "string" ? parent.sessionId : existing?.parentID,
			time: { created, updated, lastActivity: updated },
			directory: String(info.cwd ?? existing?.directory ?? this.options.directory ?? defaultCwd()),
			totalInputTokens: Number(total?.inputTokens ?? existing?.totalInputTokens ?? 0),
			totalOutputTokens: Number(total?.outputTokens ?? existing?.totalOutputTokens ?? 0),
			totalTokens: Number(total?.totalTokens ?? existing?.totalTokens ?? 0),
			totalCacheCreationTokens: Number(total?.cacheCreationInputTokens ?? existing?.totalCacheCreationTokens ?? 0),
			totalCacheReadTokens: Number(total?.cacheReadInputTokens ?? existing?.totalCacheReadTokens ?? 0),
			promptTokenEstimate: Number(total?.inputTokens ?? existing?.promptTokenEstimate ?? 0),
			lastQueryTotalTokens: existing?.lastQueryTotalTokens ?? 0,
		}
		this.sessions.set(id, session)
		this.sessionDirectories.set(id, session.directory ?? defaultCwd())
		// Durable snapshots (session/list, resume, metadata) often report Idle
		// even while a turn is live; live busy/idle rides turn/* and
		// session/statusChanged. Never downgrade a known in-flight status from
		// a snapshot — otherwise delete-refill list calls clear "working" UI.
		const snapshotBusy = String(info.status).toLowerCase() === "active"
		const existingStatus = this.sessionStatuses.get(id)
		const existingInFlight =
			existingStatus?.type === "busy" || existingStatus?.type === "retry"
		this.sessionStatuses.set(
			id,
			snapshotBusy ? { type: "busy" } : existingInFlight ? existingStatus : { type: "idle" },
		)
		return session
	}

	private async ensureInitialized(): Promise<void> {
		if (this.initialized) return
		await this.open()
		if (!this.transport) throw new Error("Devo Native transport is not connected")
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
		if (!this.transport) throw new Error("Devo Native transport is not connected")
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

	/** Native RPC path shared by all first-party Desktop consumers. */
	private async requestCanonical(method: string, params: unknown): Promise<unknown> {
		await this.ensureInitialized()
		return this.request(method, params)
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

	private handleTransportEvent(event: DevoNativeTransportEvent): void {
		if (event.type === "closed") {
			if (this.transport) {
				initializePromises.delete(this.transport)
			}
			this.initialized = false
			this.events.close()
			this.events = new AsyncEventQueue<GlobalEvent>()
			this.pendingPermissions.clear()
			this.pendingQuestions.clear()
			this.subscriptions.clear()
			this.turnSessions.clear()
			this.nativeItemCallIds.clear()
			this.referenceSearchSession = null
			return
		}
		if (event.type === "notification" && event.method && event.params) {
			if (this.handleNativeNotification(event.method, event.params)) return
		}
		if (
			event.type === "notification" &&
			event.method &&
			event.params &&
			this.referenceSearchSession?.handleNotification(event.method, event.params)
		) {
			return
		}
		if (event.type === "request" && event.id !== undefined && event.method) {
			this.handleNativeServerRequest(event.id, event.method, event.params)
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

	private handleNativeServerRequest(id: JsonRpcId, method: string, params: unknown): boolean {
		if (
			method === "approval/command/request" ||
			method === "approval/fileChange/request" ||
			method === "approval/permission/request" ||
			method === "session/goal/completionApproval/request"
		) {
			const value = objectRecord(
				this.validateTransportPayload(method, "incomingRequest", params),
			) ?? {}
			const approvalId = String(value.approvalId ?? value.requestId ?? "")
			if (!approvalId) return true
			this.pendingPermissions.set(approvalId, {
				id,
				method,
				options: [],
				availableScopes: Array.isArray(value.availableScopes)
					? value.availableScopes.map(String)
					: ["once"],
				native: true,
			})
			return true
		}
		if (method === "userInput/request") {
			const value = objectRecord(
				this.validateTransportPayload(method, "incomingRequest", params),
			) ?? {}
			const requestId = String(value.requestId ?? "")
			if (!requestId) return true
			this.pendingQuestions.set(requestId, {
				id,
				method,
				sessionId: "",
				questions: (Array.isArray(value.questions) ? value.questions : []).map(questionInfoFromNative),
			})
			return true
		}
		return false
	}

	private handleNativeNotification(method: string, params: unknown): boolean {
		const value = objectRecord(params) ?? {}
		if (method === "session/created" || method === "session/metadataUpdated") {
			const sessionValue = objectRecord(value.session)
			if (sessionValue) {
				const session = this.rememberNativeSession(sessionValue)
				this.emit(session.directory ?? defaultCwd(), {
					type: method === "session/created" ? "session.created" : "session.updated",
					properties: { info: session, session },
				})
			}
			return true
		}
		if (method === "session/statusChanged") {
			const sessionId = String(value.sessionId ?? "")
			if (!sessionId) return true
			const status = { type: String(value.status) === "active" ? "busy" : "idle" }
			this.sessionStatuses.set(sessionId, status)
			this.emit(this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
				type: "session.status",
				properties: { sessionID: sessionId, status },
			})
			return true
		}
		if (method === "session/cwdChanged") {
			const sessionId = String(value.sessionId ?? "")
			const cwd = String(value.cwd ?? "")
			if (sessionId && cwd) this.sessionDirectories.set(sessionId, cwd)
			return true
		}
		if (method === "session/deleted") {
			const deletedIds = Array.isArray(value.deletedSessionIds)
				? value.deletedSessionIds.map(String)
				: [String(value.sessionId ?? "")]
			for (const sessionId of deletedIds) {
				if (!sessionId) continue
				const { directory } = this.forgetSession(sessionId)
				this.emitSessionDeleted(sessionId, directory)
			}
			return true
		}
		if (method === "session/archived") {
			const sessionId = String(value.sessionId ?? "")
			if (sessionId && value.archived === true) {
				this.sessionStatuses.set(sessionId, { type: "idle" })
			}
			return true
		}
		if (method === "session/closed") {
			const sessionId = String(value.sessionId ?? "")
			if (sessionId) {
				this.pendingQuestions.forEach((pending, requestId) => {
					if (pending.sessionId === sessionId) this.pendingQuestions.delete(requestId)
				})
				this.pendingPermissions.forEach((pending, approvalId) => {
					if (pending.sessionId === sessionId) this.pendingPermissions.delete(approvalId)
				})
				this.subscriptions.delete(sessionId)
			}
			return true
		}
		if (method === "turn/started" || method === "turn/statusChanged" || method === "turn/completed") {
			const turn = objectRecord(value.turn)
			const turnId = String(turn?.id ?? value.turnId ?? "")
			const sessionId = String(turn?.sessionId ?? this.turnSessions.get(turnId) ?? "")
			if (!sessionId) return true
			if (turnId) this.turnSessions.set(turnId, sessionId)
			const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
			const turnStatus = String(turn?.status ?? value.status ?? "")
			const terminal = method === "turn/completed" || ["completed", "interrupted", "failed"].includes(turnStatus)
			const status = { type: terminal ? "idle" : "busy" }
			this.sessionStatuses.set(sessionId, status)
			this.emit(directory, { type: "session.status", properties: { sessionID: sessionId, status } })
			if (terminal) {
				const startedAt = this.promptStartedAtBySession.get(sessionId) ?? 0
				this.promptStartedAtBySession.delete(sessionId)
				this.completeOpenAssistantMessages(sessionId, directory, startedAt)
				this.pendingQuestions.forEach((pending, requestId) => {
					if (pending.sessionId === sessionId) this.pendingQuestions.delete(requestId)
				})
				this.pendingPermissions.forEach((pending, approvalId) => {
					if (pending.sessionId === sessionId) this.pendingPermissions.delete(approvalId)
				})
			}
			return true
		}
		if (method === "item/started" || method === "item/updated" || method === "item/completed") {
			const item = objectRecord(value.item)
			if (item) this.handleNativeItemEnvelope(item, method)
			return true
		}
		if (method === "item/assistantMessage/delta" || method === "item/reasoning/delta") {
			const sessionId = String(value.sessionId ?? "")
			const itemId = String(value.itemId ?? "")
			const delta = String(value.delta ?? "")
			if (sessionId && itemId && delta) {
				const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
				this.appendText(sessionId, directory, "assistant", method.includes("reasoning") ? "reasoning" : "text", {
					messageId: itemId,
					content: { text: delta },
				})
			}
			return true
		}
		if (method === "item/commandExecution/outputDelta") {
			const sessionId = String(value.sessionId ?? "")
			const itemId = String(value.itemId ?? "")
			const delta = String(value.delta ?? "")
			if (sessionId && itemId && delta) {
				this.appendTool(sessionId, this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
					toolCallId: this.nativeItemCallIds.get(itemId) ?? itemId,
					title: "Command",
					kind: "execute",
					status: "in_progress",
					rawOutput: delta,
				})
			}
			return true
		}
		if (method === "item/tool/requestUserInput") {
			const payload = objectRecord(value.RequestUserInput) ?? value
			const request = objectRecord(payload.request) ?? {}
			const sessionId = String(request.session_id ?? request.sessionId ?? "")
			const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
			this.handleRequestUserInput(sessionId, directory, payload)
			return true
		}
		if (method === "serverRequest/resolved") {
			const payload = objectRecord(value.ServerRequestResolved) ?? value
			const requestId = String(payload.request_id ?? payload.requestId ?? "")
			const pending = this.pendingQuestions.get(requestId)
			if (pending) {
				this.pendingQuestions.delete(requestId)
				const directory = this.sessionDirectories.get(pending.sessionId) ?? this.options.directory ?? defaultCwd()
				this.emit(directory, { type: "question.replied", properties: { sessionID: pending.sessionId, requestID: requestId } })
			}
			return true
		}
		if (method === "permission/decision") {
			const approvalId = String(value.approvalId ?? "")
			const pending = this.pendingPermissions.get(approvalId)
			if (pending) {
				this.pendingPermissions.delete(approvalId)
				this.emit(this.sessionDirectories.get(pending.sessionId ?? "") ?? this.options.directory ?? defaultCwd(), {
					type: "permission.replied",
					properties: { sessionID: pending.sessionId ?? String(value.sessionId ?? ""), requestID: approvalId },
				})
			}
			return true
		}
		if (method === "context/usageUpdated") {
			const sessionId = String(value.sessionId ?? "")
			if (sessionId) this.emitContextUsage(sessionId, value.occupancy)
			return true
		}
		if (method === "turn/usage/updated" || method === "session/usage/updated") {
			const sessionId = String(value.sessionId ?? "")
			const usage = objectRecord(value.usage) ?? {}
			const total = objectRecord(usage.total) ?? usage
			if (sessionId) {
				this.emit(this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
					type: "session.usage.updated",
					properties: {
						sessionID: sessionId,
						used: Number(total.totalTokens ?? value.lastQueryInputTokens ?? 0),
						size: Number(value.contextWindow ?? 0),
						cost: 0,
					},
				})
			}
			return true
		}
		if (method === "model/queryRetrying") {
			const sessionId = String(value.sessionId ?? "")
			const error = objectRecord(value.error) ?? {}
			if (sessionId) {
				this.emit(this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
					type: "turn.provider_retry_status",
					properties: {
						sessionID: sessionId,
						turnID: String(value.turnId ?? ""),
						attempt: Number(value.attempt ?? 0),
						backoffMs: Number(value.nextDelayMs ?? 0),
						provider: String(value.provider ?? ""),
						model: String(value.model ?? ""),
						phase: String(value.phase ?? "scheduled"),
						message: String(error.message ?? "Provider request retrying"),
					},
				})
			}
			return true
		}
		if (method === "workspace/changes/updated") {
			const payload = objectRecord(value.WorkspaceChangesUpdated) ?? value
			this.handleWorkspaceChangesUpdated(payload as WorkspaceChangesUpdatedPayload)
			return true
		}
		return false
	}

	private handleNativeItemEnvelope(envelope: Record<string, unknown>, method: string): void {
		const id = String(envelope.id ?? "")
		const sessionId = String(envelope.sessionId ?? "")
		const item = objectRecord(envelope.item)
		if (!id || !sessionId || !item) return
		const completed =
			method === "item/completed" ||
			envelope.state === "completed" ||
			envelope.state === "failed" ||
			envelope.state === "interrupted"
		const itemType = String(item.type ?? "")
		const directory = this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd()
		if (itemType === "contextCompaction" || itemType === "context_compaction") {
			const envelopeState = String(envelope.state ?? "")
			const failed = envelopeState === "failed" || item.status === "failed"
			const status = failed ? "failed" : completed || envelopeState === "completed" ? "completed" : "started"
			this.emit(directory, {
				type: `session.compaction.${status}`,
				properties: { sessionID: sessionId },
			})
			this.upsertCompaction(sessionId, directory, {
				itemId: id,
				status,
				turnId: String(envelope.turnId ?? ""),
			})
			if (completed) this.renderedNativeItems.add(id)
			return
		}
		if (itemType === "plan") {
			this.upsertPlan(sessionId, directory, id, item, String(envelope.turnId ?? ""))
			if (completed) this.renderedNativeItems.add(id)
			return
		}
		if (completed && this.renderedNativeItems.has(id)) return

		if (itemType === "approval") {
			const approvalId = String(item.approvalId ?? "")
			const pending = this.pendingPermissions.get(approvalId)
			if (pending) {
				pending.sessionId = sessionId
				pending.availableScopes = Array.isArray(item.availableScopes) ? item.availableScopes.map(String) : pending.availableScopes
			}
			if (item.decision) {
				this.pendingPermissions.delete(approvalId)
				this.emit(directory, { type: "permission.replied", properties: { sessionID: sessionId, requestID: approvalId } })
			} else if (pending) {
				const target = objectRecord(item.target)
				const targetKind = String(target?.kind ?? "")
				this.emit(directory, {
					type: "permission.asked",
					properties: {
						id: approvalId,
						requestID: approvalId,
						sessionID: sessionId,
						permission: String(item.actionSummary ?? "Agent requested permission"),
						metadata: {
							tool: item.resource,
							command: targetKind === "command" ? target?.command : undefined,
							path: targetKind === "path" ? target?.path : undefined,
							host: targetKind === "host" ? target?.host : undefined,
							justification: item.justification,
							resource: item.resource,
							target: target?.command ?? target?.path ?? target?.host,
							availableScopes: pending.availableScopes,
							commandPattern: item.commandPattern,
							commandPrefix: item.commandPrefix,
						},
					},
				})
			}
			return
		}
		if (itemType === "userInputRequest") {
			const requestId = String(item.requestId ?? "")
			const pending = this.pendingQuestions.get(requestId)
			if (item.answers || completed) {
				this.pendingQuestions.delete(requestId)
				if (pending) {
					this.emit(directory, {
						type: "question.replied",
						properties: { sessionID: pending.sessionId || sessionId, requestID: requestId },
					})
				}
			} else if (pending) {
				pending.sessionId = sessionId
				pending.questions = (Array.isArray(item.questions) ? item.questions : []).map(questionInfoFromNative)
				this.emit(directory, { type: "question.asked", properties: { id: requestId, requestID: requestId, sessionID: sessionId, questions: pending.questions } })
			}
			return
		}
		if (itemType === "assistantMessage" || itemType === "reasoning") {
			const partType = itemType === "reasoning" ? "reasoning" : "text"
			const existing = this.parts.get(partCacheKey(sessionId, id))?.some((part) => part.type === partType)
			if (!existing) this.appendText(sessionId, directory, "assistant", partType, { messageId: id, content: { text: String(item.text ?? "") } })
		} else if (itemType === "userMessage") {
			const text = (Array.isArray(item.content) ? item.content : []).map((part) => objectRecord(part)).filter(Boolean).filter((part) => part?.type === "text").map((part) => String(part?.text ?? "")).join("\n")
			const existing = this.parts.get(partCacheKey(sessionId, id))?.some((part) => part.type === "text")
			if (!existing) this.appendText(sessionId, directory, "user", "text", { messageId: id, content: { text } })
		} else if (
			itemType === "toolCall" ||
			itemType === "commandExecution" ||
			itemType === "toolResult" ||
			itemType === "fileChange" ||
			itemType === "hostedToolCall"
		) {
			if (item.callId) this.nativeItemCallIds.set(id, String(item.callId))
			this.appendTool(sessionId, directory, {
				toolCallId: item.callId,
				title: item.toolName ?? item.command ?? (itemType === "fileChange" ? "Write" : "Tool"),
				...(item.toolName
					? { kind: item.toolName }
					: itemType === "commandExecution"
						? { kind: "execute" }
						: {}),
				status: completed ? (item.isError || envelope.state === "failed" ? "failed" : "completed") : "in_progress",
				rawInput: item.input ?? fileChangeInput(item),
				rawOutput: item.output,
			})
		}
		if (completed) this.renderedNativeItems.add(id)
	}

	private async ensureSessionSubscription(sessionId: string): Promise<void> {
		if (this.subscriptions.has(sessionId)) return
		const after = this.subscriptionCursors.get(sessionId) ?? []
		const result = (await this.requestCanonical("subscription/create", {
			selectors: [{ kind: "session", sessionId }],
			includeSnapshot: true,
			after,
		})) as {
			subscriptionId: string
			snapshots?: Array<Record<string, unknown>>
			replay?: Array<Record<string, unknown>>
			cursors?: Array<{ streamId: string; seq: number }>
			pendingControlRequests?: Array<Record<string, unknown>>
		}
		const cursors = result.cursors ?? []
		this.subscriptions.set(sessionId, { subscriptionId: result.subscriptionId, cursors })
		this.subscriptionCursors.set(sessionId, cursors)
		for (const snapshot of result.snapshots ?? []) {
			const data = objectRecord(snapshot.data)
			const session = objectRecord(data?.session)
			if (session) this.rememberNativeSession(session)
		}
		for (const envelope of result.replay ?? []) {
			const notification = objectRecord(envelope.notification)
			if (notification && typeof notification.method === "string") {
				this.handleNativeNotification(notification.method, notification.params)
			}
		}
		for (const pending of result.pendingControlRequests ?? []) {
			const item = objectRecord(pending.item)
			if (item) this.handleNativeItemEnvelope(item, "item/started")
		}
		if (cursors.length > 0) {
			await this.requestCanonical("subscription/ack", { subscriptionId: result.subscriptionId, cursors })
		}
	}

	private async ensureKnownSessionSubscriptions(): Promise<void> {
		const sessions = await this.listSessions()
		for (const session of sessions) await this.ensureSessionSubscription(session.id)
	}

	private async respondToPermission(
		permissionId: string,
		response: PermissionResponse,
	): Promise<void> {
		await this.open()
		if (!this.transport) throw new Error("Devo Native transport is not connected")
		const pending = this.pendingPermissions.get(permissionId)
		if (!pending) return
		this.pendingPermissions.delete(permissionId)
		const scopes = pending.availableScopes ?? ["once"]
		const requestedScope = response === "always"
			? ["commandPrefixPersist", "commandPrefix", "pathPrefix", "host", "tool", "session", "turn", "once"]
				.find((candidate) => scopes.includes(candidate)) ?? "once"
			: response === "reject" ? "once" : response
		const scope = scopes.includes(requestedScope) ? requestedScope : "once"
		const result = assertValidProtocolPayload({
			method: pending.method,
			direction: "outgoingResponse",
			payload: {
				requestId: permissionId,
				decision: {
					decision: response === "reject" ? "denied" : "approved",
					scope,
					decisionSource: "user",
					decidedAt: new Date().toISOString(),
				},
			},
		})
		await this.transport.respond(pending.id, result)
		this.emit(
			this.sessionDirectories.get(pending.sessionId ?? "") ?? this.options.directory ?? defaultCwd(),
			{
				type: "permission.replied",
				properties: {
					sessionID: pending.sessionId ?? "",
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
		if (!this.transport) throw new Error("Devo Native transport is not connected")
		const result = assertValidProtocolPayload({
			method: pending.method ?? "userInput/request",
			direction: "outgoingResponse",
			payload: {
				requestId,
				answers: responseAnswers,
			},
		})
		await this.transport.respond(pending.id, result)
		this.pendingQuestions.delete(requestId)
		this.emit(this.sessionDirectories.get(pending.sessionId) ?? this.options.directory ?? defaultCwd(), {
			type: eventType,
			properties: {
				sessionID: pending.sessionId,
				requestID: requestId,
			},
		})
	}

	private handleSessionUpdate(notification: LegacySessionNotification): void {
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
					this.rememberConfigOptions(sessionId, directory, update.configOptions as SessionConfigOption[])
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
		notification: LegacySessionNotification,
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
			if (compaction.itemId && compaction.status !== "failed") {
				this.upsertCompaction(sessionId, directory, {
					itemId: compaction.itemId,
					status: compaction.status,
					turnId: compaction.turnId,
				})
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
		this.promptStartedAtBySession.delete(sessionId)
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
		const questions = rawQuestions.map(questionInfoFromNative)
		const existing = this.pendingQuestions.get(requestId)
		this.pendingQuestions.set(requestId, {
			id: existing?.id,
			method: existing?.method,
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

	private upsertCompaction(
		sessionId: string,
		directory: string,
		update: {
			itemId: string
			status: "started" | "completed" | "failed"
			turnId?: string
		},
	): void {
		if (update.status === "failed") return
		const messageId = `compaction-${update.itemId}`
		const label = update.status === "completed" ? COMPACTION_COMPLETED_LABEL : COMPACTION_STARTED_LABEL
		this.replaceText(sessionId, directory, "assistant", {
			messageId,
			content: { text: label },
			_meta: {
				[DEVO_ITEM_KIND_META]: "context_compaction",
				[DEVO_COMPACTION_STATUS_META]: update.status,
				...(update.turnId ? { [DEVO_TURN_ID_META]: update.turnId } : {}),
			},
		})
	}

	private upsertPlan(
		sessionId: string,
		directory: string,
		itemId: string,
		item: Record<string, unknown>,
		turnId: string,
	): void {
		const entries = Array.isArray(item.entries) ? item.entries : []
		const mapped = entries.map((entry) => {
			const value = objectRecord(entry) ?? {}
			return {
				content: String(value.step ?? value.content ?? value.title ?? ""),
				status: String(value.status ?? "pending"),
			}
		})
		if (mapped.length === 0) return
		this.emit(directory, {
			type: "todo.updated",
			properties: { sessionID: sessionId, todos: mapped },
		})
		const proposed = mapped.length === 1 && mapped[0].content.includes("\n")
		this.replaceText(sessionId, directory, "assistant", {
			messageId: itemId,
			content: {
				text: proposed
					? mapped[0].content
					: mapped.map((entry) => `- [${entry.status}] ${entry.content}`).join("\n"),
			},
			_meta: {
				[DEVO_ITEM_KIND_META]: proposed ? "proposed_plan" : "plan",
				planEntries: mapped,
				...(turnId ? { [DEVO_TURN_ID_META]: turnId } : {}),
			},
		})
	}

	private replaceText(
		sessionId: string,
		directory: string,
		role: "assistant" | "user",
		update: Record<string, unknown>,
	): void {
		const text = textFromUpdate(update)
		if (!text) return
		const now = this.nextEventTime()
		const messageId =
			typeof update.messageId === "string" ? update.messageId : `${role}-${sessionId}-${now}`
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

		const partId = `${messageId}-text`
		const existingPart = this.parts
			.get(partCacheKey(sessionId, messageId))
			?.find((part) => part.id === partId)
		const partEventTime = updateHistoryCreatedAt(update) ?? now
		const metadata = textPartMetadataFromUpdate(update, existingPart)
		const part = {
			id: partId,
			sessionID: sessionId,
			messageID: messageId,
			type: "text",
			text,
			...(metadata ? { metadata } : {}),
			time: partTime(existingPart, partEventTime),
		} as TextPart
		this.appendPart(sessionId, messageId, part)
		this.emit(directory, { type: "message.part.updated", properties: { part } })
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
		const nextText =
			role === "user" && existingText && (existingText === text || existingText.endsWith(text))
				? existingText
				: role === "user" && existingText && text.startsWith(existingText)
					? text
					: `${existingText}${text}`
		if (existingPart && nextText === existingText) return
		const partEventTime = updateHistoryCreatedAt(update) ?? now
		const metadata = textPartMetadataFromUpdate(update, existingPart)
		const part = {
			id: partId,
			sessionID: sessionId,
			messageID: messageId,
			type: partType,
			[field]: nextText,
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
			this.completeInFlightToolParts(sessionId, directory, updated.id, completedAt)
		}
	}

	private completeInFlightToolParts(
		sessionId: string,
		directory: string,
		messageId: string,
		completedAt: number,
	): void {
		const parts = this.parts.get(partCacheKey(sessionId, messageId))
		if (!parts) return
		for (let index = 0; index < parts.length; index++) {
			const part = parts[index]
			if (part.type !== "tool") continue
			if (part.state.status !== "running" && part.state.status !== "pending") continue
			const runningOutput =
				part.state.status === "running" && typeof part.state.metadata?.output === "string"
					? part.state.metadata.output
					: ""
			const next = {
				...part,
				state: {
					...part.state,
					status: "completed",
					output: runningOutput,
					time: { ...part.state.time, end: part.state.time.end ?? completedAt },
				},
			} as ToolPart
			parts[index] = next
			this.emit(directory, { type: "message.part.updated", properties: { part: next } })
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
		configOptions?: SessionConfigOption[],
	): void {
		if (!Array.isArray(configOptions)) return
		this.configOptionsBySession.set(sessionId, configOptions)
		this.rememberDirectoryConfigOptions(directory, configOptions)
	}

	private rememberDirectoryConfigOptions(
		directory: string,
		configOptions?: SessionConfigOption[] | null,
	): void {
		if (!Array.isArray(configOptions)) return
		this.configOptionsByDirectory.set(directory, configOptions)
	}

	private async setSessionConfigOption(
		sessionId: string,
		configId: string,
		value: string,
	): Promise<void> {
		const update: Record<string, unknown> = {
			sessionId,
			expectedVersion: 0,
		}
		if (configId === "model") {
			update.model = { provider: "", model: value }
		} else if (configId === "thought_level") {
			update.settings = { reasoningEffort: value }
		} else if (configId === "mode") {
			update.settings = { mode: value }
		} else {
			throw new Error(`unknown session config option '${configId}'`)
		}
		await this.requestCanonical("session/metadata/update", update)
	}

	private async setDefaultConfigOption(
		configId: string,
		value: string,
	): Promise<SessionConfigOption[]> {
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
		const options = sessionConfigOptionsFromModelPreferences(result.preferences ?? {})
		this.rememberDirectoryConfigOptions(directory, options)
		return this.currentConfigOptions()
	}

	private cachedConfigOptions(): SessionConfigOption[] | undefined {
		if (this.options.directory) {
			const byDirectory = this.configOptionsByDirectory.get(this.options.directory)
			if (byDirectory) return byDirectory
		}
		return this.configOptionsBySession.values().next().value
	}

	private currentConfigOptions(): SessionConfigOption[] {
		return this.cachedConfigOptions() ?? []
	}

	invalidateConfigOptionCaches(): void {
		this.configOptionsBySession.clear()
		this.configOptionsByDirectory.clear()
	}

	private async ensureCurrentConfigOptions(): Promise<SessionConfigOption[]> {
		const cached = this.cachedConfigOptions()
		if (cached) return cached

		await this.ensureInitialized()
		const directory = this.options.directory ?? defaultCwd()
		// Canonical model/preferences/read (ratified #12).
		const params: Record<string, unknown> = this.options.directory ? { cwd: this.options.directory } : {}
		const result = (await this.requestCanonical("model/preferences/read", params)) as {
			preferences?: ModelPreferencesWire
		}
		this.rememberDirectoryConfigOptions(directory, sessionConfigOptionsFromModelPreferences(result.preferences ?? {}))
		return this.currentConfigOptions()
	}

	private emitContextUsage(sessionId: string, occupancyValue: unknown): boolean {
		const occupancy = contextOccupancyFromProtocol(occupancyValue)
		if (!occupancy) return false
		this.emit(this.sessionDirectories.get(sessionId) ?? this.options.directory ?? defaultCwd(), {
			type: "context.usage.updated",
			properties: { sessionID: sessionId, occupancy },
		})
		return true
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
	return new NativeClient(options)
}

function sessionIdFromPayload(payload: unknown): string | null {
	if (!payload || typeof payload !== "object") return null
	const value = payload as Record<string, unknown>
	for (const key of ["sessionId", "session_id"]) {
		if (typeof value[key] === "string") return value[key] as string
	}
	return null
}
