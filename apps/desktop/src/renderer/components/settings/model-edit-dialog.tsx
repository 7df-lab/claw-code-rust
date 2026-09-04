/**
 * Model editor dialog — basic fields + collapsible advanced settings.
 *
 * Wire API is a model-level property (overrides provider default).
 * Model ID is editable; renaming rewrites the nested models map key.
 * Persists via provider/upsert.
 */

import type {
	CatalogModelInfo,
	CatalogProviderInfo,
	CatalogWireApi,
	InputModality,
	ProviderModelVariant,
	ReasoningCapability,
	ReasoningEffort,
	ReasoningLevelChoice,
} from "@devo-ai/sdk/v2/client"
import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { Label } from "@devo/ui/components/label"
import {
	Select,
	SelectContent,
	SelectGroup,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { Spinner } from "@devo/ui/components/spinner"
import { Switch } from "@devo/ui/components/switch"
import { Textarea } from "@devo/ui/components/textarea"
import { cn } from "@devo/ui/lib/utils"
import { ChevronDownIcon, PlusIcon, Trash2Icon } from "lucide-react"
import { useCallback, useMemo, useState } from "react"
import { getBaseClient } from "../../services/connection-manager"
import { effectiveContextWindowTokens, contextWindowPercentFromAbsolute } from "../../lib/providers"

const WIRE_API_OPTIONS: Array<{ value: CatalogWireApi; label: string }> = [
	{ value: "openai_chat_completions", label: "OpenAI Chat Completions" },
	{ value: "openai_responses", label: "OpenAI Responses" },
	{ value: "anthropic_messages", label: "Anthropic Messages" },
]

const MODALITY_OPTIONS: InputModality[] = ["text", "image"]
const MODEL_ID_PATTERN = /^[a-z0-9][a-z0-9._:/-]*$/i
const EFFORT_LEVELS: ReasoningEffort[] = ["none", "minimal", "low", "medium", "high", "xhigh", "max"]
const LEVEL_CHOICES: ReasoningLevelChoice[] = ["off", ...EFFORT_LEVELS]

interface HeaderEntry {
	key: string
	value: string
}

type ReasoningMode = "unsupported" | "toggle" | "levels"

interface EffortEncodingDraft {
	requestModel: string
	requestBody: string
	headers: HeaderEntry[]
}

function isReasoningLevelChoice(value: string): value is ReasoningLevelChoice {
	return value === "off" || EFFORT_LEVELS.includes(value as ReasoningEffort)
}

function capabilityMode(capability: ReasoningCapability | null | undefined): ReasoningMode {
	if (capability == null || capability === "unsupported") return "unsupported"
	if (capability === "toggle") return "toggle"
	if (typeof capability === "object" && "levels" in capability) return "levels"
	// Legacy wire form before server/protocol migration.
	if (typeof capability === "object" && "toggle_with_levels" in (capability as object)) {
		return "levels"
	}
	return "unsupported"
}

function capabilityLevels(
	capability: ReasoningCapability | null | undefined,
): ReasoningLevelChoice[] {
	if (capability == null || typeof capability !== "object") return []
	if ("levels" in capability) {
		return capability.levels.filter(isReasoningLevelChoice)
	}
	const legacy = (capability as { toggle_with_levels?: ReasoningEffort[] }).toggle_with_levels
	if (Array.isArray(legacy)) {
		const levels: ReasoningLevelChoice[] = ["off"]
		for (const effort of legacy) {
			if (!levels.includes(effort)) levels.push(effort)
		}
		return levels
	}
	return []
}

function buildReasoningCapability(
	mode: ReasoningMode,
	levels: ReasoningLevelChoice[],
): ReasoningCapability | undefined {
	switch (mode) {
		case "unsupported":
			return "unsupported"
		case "toggle":
			return "toggle"
		case "levels":
			return { levels: levels.length > 0 ? levels : ["medium"] }
	}
}

function effortOptionValues(mode: ReasoningMode, levels: ReasoningLevelChoice[]): string[] {
	switch (mode) {
		case "unsupported":
			return []
		case "toggle":
			return ["off", "on"]
		case "levels":
			return levels
	}
}

function emptyEffortEncoding(): EffortEncodingDraft {
	return { requestModel: "", requestBody: "", headers: [] }
}

function encodingFromVariant(variant: ProviderModelVariant | undefined): EffortEncodingDraft {
	if (!variant) return emptyEffortEncoding()
	return {
		requestModel: variant.requestModel ?? "",
		requestBody: formatJsonValue(variant.request),
		headers: Object.entries(variant.headers ?? {}).map(([key, value]) => ({ key, value })),
	}
}

function encodingIsEmpty(encoding: EffortEncodingDraft): boolean {
	return (
		!encoding.requestModel.trim() &&
		!encoding.requestBody.trim() &&
		!encoding.headers.some((header) => header.key.trim())
	)
}

function parseEffortEncoding(encoding: EffortEncodingDraft): ProviderModelVariant | undefined {
	if (encodingIsEmpty(encoding)) return undefined
	const nextHeaders: Record<string, string> = {}
	for (const header of encoding.headers) {
		const key = header.key.trim()
		if (!key) continue
		nextHeaders[key] = header.value
	}
	return {
		requestModel: encoding.requestModel.trim() || undefined,
		request: parseJsonObject(encoding.requestBody),
		headers: Object.keys(nextHeaders).length > 0 ? nextHeaders : undefined,
	}
}

function wireLabel(api: CatalogWireApi | undefined, fallback?: CatalogWireApi): string {
	const value = api ?? fallback
	if (!value) return "—"
	return WIRE_API_OPTIONS.find((o) => o.value === value)?.label ?? value.replace(/_/g, " ")
}

function parseOptionalNumber(raw: string): number | undefined {
	const trimmed = raw.trim()
	if (!trimmed) return undefined
	const n = Number(trimmed)
	return Number.isFinite(n) ? n : undefined
}

function formatJsonValue(value: unknown): string {
	if (value == null) return ""
	try {
		return JSON.stringify(value, null, 2)
	} catch {
		return ""
	}
}

/** Empty string clears the field; otherwise require a JSON object. */
function parseJsonObject(raw: string): unknown | undefined {
	const trimmed = raw.trim()
	if (!trimmed) return undefined
	const parsed: unknown = JSON.parse(trimmed)
	if (parsed === null || typeof parsed !== "object" || Array.isArray(parsed)) {
		throw new Error("Request body must be a JSON object")
	}
	return parsed
}

export function effectiveWireApi(
	model: CatalogModelInfo,
	provider: CatalogProviderInfo,
): CatalogWireApi {
	return model.wireApi ?? provider.wireApis[0] ?? "openai_chat_completions"
}

interface ModelEditDialogProps {
	provider: CatalogProviderInfo
	/** Omit or leave empty when creating a new model. */
	modelId?: string
	model?: CatalogModelInfo
	mode?: "edit" | "create"
	open: boolean
	onOpenChange: (open: boolean) => void
	onSaved: () => void
}

export function ModelEditDialog({
	provider,
	modelId: originalModelId = "",
	model = {},
	mode = "edit",
	open,
	onOpenChange,
	onSaved,
}: ModelEditDialogProps) {
	const isCreate = mode === "create"
	const providerDefaultWire = provider.wireApis[0] ?? "openai_chat_completions"
	const existingModels = provider.models ?? {}

	const [modelId, setModelId] = useState(originalModelId)
	const [displayName, setDisplayName] = useState(model.name ?? "")
	const [wireApi, setWireApi] = useState<CatalogWireApi>(
		isCreate ? providerDefaultWire : effectiveWireApi(model, provider),
	)
	const [enabled, setEnabled] = useState(model.enabled !== false)
	const [advancedOpen, setAdvancedOpen] = useState(false)

	const [contextWindow, setContextWindow] = useState(() => {
		const effective = effectiveContextWindowTokens(model)
		return effective != null ? String(effective) : ""
	})
	const [maxTokens, setMaxTokens] = useState(model.maxTokens != null ? String(model.maxTokens) : "")
	const [temperature, setTemperature] = useState(
		model.temperature != null ? String(model.temperature) : "",
	)
	const [topP, setTopP] = useState(model.topP != null ? String(model.topP) : "")
	const [topK, setTopK] = useState(model.topK != null ? String(model.topK) : "")
	const [modalities, setModalities] = useState<InputModality[]>(
		model.inputModalities?.length ? [...model.inputModalities] : ["text"],
	)
	const [headers, setHeaders] = useState<HeaderEntry[]>(() =>
		Object.entries(model.headers ?? {}).map(([key, value]) => ({ key, value })),
	)
	const [requestBody, setRequestBody] = useState(() => formatJsonValue(model.request))
	const [optionsBody, setOptionsBody] = useState(() => formatJsonValue(model.options))
	const [reasoningMode, setReasoningMode] = useState<ReasoningMode>(() =>
		capabilityMode(model.reasoningCapability),
	)
	const [reasoningLevels, setReasoningLevels] = useState<ReasoningLevelChoice[]>(() =>
		capabilityLevels(model.reasoningCapability),
	)
	const [defaultReasoningSelection, setDefaultReasoningSelection] = useState(
		() => model.defaultReasoningSelection ?? "",
	)
	const [effortEncodings, setEffortEncodings] = useState<Record<string, EffortEncodingDraft>>(
		() => {
			const next: Record<string, EffortEncodingDraft> = {}
			for (const [key, variant] of Object.entries(model.variants ?? {})) {
				next[key] = encodingFromVariant(variant)
			}
			return next
		},
	)

	const [saving, setSaving] = useState(false)
	const [error, setError] = useState<string | null>(null)

	const addHeader = useCallback(() => {
		setHeaders((prev) => [...prev, { key: "", value: "" }])
	}, [])

	const removeHeader = useCallback((index: number) => {
		setHeaders((prev) => prev.filter((_, i) => i !== index))
	}, [])

	const updateHeader = useCallback((index: number, field: "key" | "value", value: string) => {
		setHeaders((prev) => prev.map((h, i) => (i === index ? { ...h, [field]: value } : h)))
	}, [])

	const toggleModality = useCallback((mod: InputModality) => {
		setModalities((prev) => {
			if (prev.includes(mod)) {
				const next = prev.filter((m) => m !== mod)
				return next.length > 0 ? next : prev
			}
			return [...prev, mod]
		})
	}, [])

	const toggleReasoningLevel = useCallback((level: ReasoningLevelChoice) => {
		setReasoningLevels((prev) => {
			if (prev.includes(level)) return prev.filter((item) => item !== level)
			return [...prev, level]
		})
	}, [])

	const effortOptions = useMemo(
		() => effortOptionValues(reasoningMode, reasoningLevels),
		[reasoningMode, reasoningLevels],
	)

	const updateEffortEncoding = useCallback(
		(selection: string, patch: Partial<EffortEncodingDraft>) => {
			setEffortEncodings((prev) => ({
				...prev,
				[selection]: {
					...(prev[selection] ?? emptyEffortEncoding()),
					...patch,
				},
			}))
		},
		[],
	)

	const handleSave = useCallback(async () => {
		const nextId = modelId.trim()
		if (!nextId) {
			setError("Model ID is required")
			return
		}
		if (!MODEL_ID_PATTERN.test(nextId)) {
			setError("Model ID may use letters, numbers, and . _ : / -")
			return
		}
		if (nextId !== originalModelId && existingModels[nextId]) {
			setError(`Model ID "${nextId}" already exists on this provider`)
			return
		}
		if (isCreate && existingModels[nextId]) {
			setError(`Model ID "${nextId}" already exists on this provider`)
			return
		}

		let parsedRequest: unknown | undefined
		let parsedOptions: unknown | undefined
		try {
			parsedRequest = parseJsonObject(requestBody)
			parsedOptions = parseJsonObject(optionsBody)
		} catch (err) {
			setError(err instanceof Error ? err.message : "Invalid JSON")
			return
		}

		const nextHeaders: Record<string, string> = {}
		for (const header of headers) {
			const key = header.key.trim()
			if (!key) continue
			nextHeaders[key] = header.value
		}

		const nextVariants: Record<string, ProviderModelVariant> = {
			...(model.variants ?? {}),
		}
		for (const selection of effortOptions) {
			try {
				const parsed = parseEffortEncoding(effortEncodings[selection] ?? emptyEffortEncoding())
				if (parsed) nextVariants[selection] = parsed
				else delete nextVariants[selection]
			} catch (err) {
				setError(
					err instanceof Error
						? `Effort encoding "${selection}": ${err.message}`
						: `Invalid effort encoding for ${selection}`,
				)
				return
			}
		}

		const capability = buildReasoningCapability(reasoningMode, reasoningLevels)
		const normalizedDefault = defaultReasoningSelection.trim().toLowerCase()
		if (
			normalizedDefault &&
			effortOptions.length > 0 &&
			!effortOptions.includes(normalizedDefault) &&
			normalizedDefault !== "enabled" &&
			normalizedDefault !== "disabled"
		) {
			setError(`Default reasoning must be one of: ${effortOptions.join(", ")}`)
			return
		}

		setSaving(true)
		setError(null)
		try {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")

			const userContextTokens = parseOptionalNumber(contextWindow)
			const hardWindow = model.contextWindow
			let nextContextWindow: number | undefined
			let nextPercent: number | undefined
			if (userContextTokens == null) {
				// Clear percent overlay → default 95% of hard capacity.
				nextContextWindow = hardWindow ?? undefined
				nextPercent = undefined
			} else if (hardWindow != null && hardWindow > 0) {
				nextContextWindow = hardWindow
				nextPercent = contextWindowPercentFromAbsolute(hardWindow, userContextTokens)
			} else {
				// Custom model with no hard window yet: treat entry as hard @ 100%.
				nextContextWindow = userContextTokens
				nextPercent = 100
			}
			const nextModel: CatalogModelInfo = {
				...model,
				name: displayName.trim() || undefined,
				wireApi,
				enabled,
				contextWindow: nextContextWindow,
				effectiveContextWindowPercent: nextPercent,
				maxTokens: parseOptionalNumber(maxTokens),
				temperature: parseOptionalNumber(temperature),
				topP: parseOptionalNumber(topP),
				topK: parseOptionalNumber(topK),
				inputModalities: modalities.length > 0 ? modalities : undefined,
				headers: Object.keys(nextHeaders).length > 0 ? nextHeaders : undefined,
				request: parsedRequest,
				options: parsedOptions,
				reasoningCapability: capability,
				defaultReasoningSelection: normalizedDefault
					? normalizedDefault === "enabled"
						? "on"
						: normalizedDefault === "disabled"
							? "off"
							: normalizedDefault
					: undefined,
				defaultReasoningEffort:
					normalizedDefault && EFFORT_LEVELS.includes(normalizedDefault as ReasoningEffort)
						? (normalizedDefault as ReasoningEffort)
						: model.defaultReasoningEffort,
				variants: Object.keys(nextVariants).length > 0 ? nextVariants : undefined,
			}

			const wireApis = provider.wireApis.includes(wireApi)
				? provider.wireApis
				: [...provider.wireApis, wireApi]

			const updatedModels: Record<string, CatalogModelInfo> = {
				[nextId]: nextModel,
			}

			await client.provider.upsert({
				provider: {
					...provider,
					wireApis: wireApis.length > 0 ? wireApis : [wireApi],
					// Server merges models by insert; send only this model so we
					// do not copy the full template+connection catalog into the overlay.
					models: updatedModels,
				},
			})

			// Upsert never deletes missing keys — rename must remove the old id.
			if (!isCreate && nextId !== originalModelId) {
				await client.provider.modelRemove({
					providerId: provider.id,
					modelId: originalModelId,
				})
			}

			onOpenChange(false)
			onSaved()
		} catch (err) {
			setError(err instanceof Error ? err.message : "Failed to save model")
		} finally {
			setSaving(false)
		}
	}, [
		modelId,
		originalModelId,
		existingModels,
		isCreate,
		model,
		displayName,
		wireApi,
		enabled,
		contextWindow,
		maxTokens,
		temperature,
		topP,
		topK,
		modalities,
		headers,
		requestBody,
		optionsBody,
		reasoningMode,
		reasoningLevels,
		defaultReasoningSelection,
		effortEncodings,
		effortOptions,
		provider,
		onOpenChange,
		onSaved,
	])

	const advancedSummary = useMemo(() => {
		const parts: string[] = []
		if (reasoningMode !== "unsupported") parts.push(`reasoning:${reasoningMode}`)
		if (contextWindow.trim()) parts.push(`${Math.round(Number(contextWindow) / 1000)}k ctx`)
		if (modalities.length) parts.push(modalities.join("+"))
		const headerCount = headers.filter((h) => h.key.trim()).length
		if (headerCount > 0) parts.push(`${headerCount} header${headerCount === 1 ? "" : "s"}`)
		if (requestBody.trim()) parts.push("request body")
		if (optionsBody.trim()) parts.push("options")
		const encodingCount = effortOptions.filter(
			(selection) => !encodingIsEmpty(effortEncodings[selection] ?? emptyEffortEncoding()),
		).length
		if (encodingCount > 0) parts.push(`${encodingCount} effort encoding${encodingCount === 1 ? "" : "s"}`)
		return parts.join(" · ")
	}, [
		reasoningMode,
		contextWindow,
		modalities,
		headers,
		requestBody,
		optionsBody,
		effortOptions,
		effortEncodings,
	])

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="sm:max-w-2xl gap-0 p-0 overflow-hidden">
				<div className="border-b border-border/50 px-6 py-4">
					<DialogHeader className="gap-1">
						<DialogTitle className="text-base font-medium tracking-tight">
							{isCreate ? "Add model" : "Edit model"}
						</DialogTitle>
						<DialogDescription className="text-sm text-muted-foreground">
							{provider.name}
							{!isCreate &&
								(originalModelId !== modelId.trim() && modelId.trim() ? (
									<>
										{" "}
										· renaming{" "}
										<span className="font-mono text-[12px]">{originalModelId}</span>
										{" → "}
										<span className="font-mono text-[12px]">{modelId.trim()}</span>
									</>
								) : (
									<>
										{" "}
										· <span className="font-mono text-[12px]">{originalModelId}</span>
									</>
								))}
						</DialogDescription>
					</DialogHeader>
				</div>

				<div className="flex max-h-[min(75vh,640px)] flex-col gap-5 overflow-y-auto px-6 py-5">
					<div className="grid gap-3.5 sm:grid-cols-2">
						<div className="flex flex-col gap-1.5 sm:col-span-2">
							<Label htmlFor="model-id" className="text-xs text-muted-foreground">
								Model ID
							</Label>
							<Input
								id="model-id"
								value={modelId}
								onChange={(e) => setModelId(e.target.value)}
								placeholder="provider-facing-model-id"
								disabled={saving}
								className="h-9 font-mono text-[13px]"
							/>
							<p className="text-[11px] text-muted-foreground">
								{isCreate
									? "Sent to the provider as the request model."
									: "Sent to the provider as the request model. Changing it renames this entry."}
							</p>
						</div>

						<div className="flex flex-col gap-1.5 sm:col-span-2">
							<Label htmlFor="model-name" className="text-xs text-muted-foreground">
								Display name
							</Label>
							<Input
								id="model-name"
								value={displayName}
								onChange={(e) => setDisplayName(e.target.value)}
								placeholder={modelId.trim() || originalModelId}
								disabled={saving}
								className="h-9"
							/>
						</div>

						<div className="flex flex-col gap-1.5 sm:col-span-2">
							<Label className="text-xs text-muted-foreground">Invocation method</Label>
							<Select
								value={wireApi}
								onValueChange={(v) => {
									if (v != null) setWireApi(v as CatalogWireApi)
								}}
								disabled={saving}
							>
								<SelectTrigger className="h-9">
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									<SelectGroup>
										{WIRE_API_OPTIONS.map((opt) => (
											<SelectItem key={opt.value} value={opt.value}>
												{opt.label}
												{opt.value === providerDefaultWire ? " (provider default)" : ""}
											</SelectItem>
										))}
									</SelectGroup>
								</SelectContent>
							</Select>
							<p className="text-[11px] text-muted-foreground">
								Per-model override. Provider default is {wireLabel(undefined, providerDefaultWire)}.
							</p>
						</div>

						<div className="flex items-center justify-between rounded-lg border border-border/50 px-3 py-2.5 sm:col-span-2">
							<div>
								<p className="text-sm tracking-tight">Enabled</p>
								<p className="text-xs text-muted-foreground">Show this model in the picker</p>
							</div>
							<Switch checked={enabled} onCheckedChange={setEnabled} disabled={saving} />
						</div>

						<div className="flex flex-col gap-1.5 sm:col-span-2">
							<Label className="text-xs text-muted-foreground">Reasoning capability</Label>
							<Select
								value={reasoningMode}
								onValueChange={(value) => {
									if (value == null) return
									const mode = value as ReasoningMode
									setReasoningMode(mode)
									if (mode === "levels" && reasoningLevels.length === 0) {
										setReasoningLevels(["medium"])
									}
								}}
								disabled={saving}
							>
								<SelectTrigger className="h-9">
									<SelectValue />
								</SelectTrigger>
								<SelectContent>
									<SelectGroup>
										<SelectItem value="unsupported">Unsupported</SelectItem>
										<SelectItem value="toggle">Toggle (off / on)</SelectItem>
										<SelectItem value="levels">Levels</SelectItem>
									</SelectGroup>
								</SelectContent>
							</Select>
							<p className="text-[11px] text-muted-foreground">
								Controls the effort chips in chat. Include <code>off</code> in levels to allow
								disabling. Encoding for custom gateways is configured under Advanced.
							</p>
						</div>

						{reasoningMode === "levels" && (
							<div className="flex flex-col gap-1.5 sm:col-span-2">
								<Label className="text-xs text-muted-foreground">Reasoning levels</Label>
								<div className="flex flex-wrap gap-1.5">
									{LEVEL_CHOICES.map((level) => {
										const active = reasoningLevels.includes(level)
										return (
											<button
												key={level}
												type="button"
												disabled={saving}
												onClick={() => toggleReasoningLevel(level)}
												className={cn(
													"h-7 rounded-md px-2.5 text-xs transition-colors",
													active
														? "bg-muted font-medium text-foreground"
														: "bg-transparent text-muted-foreground hover:bg-muted/60",
												)}
											>
												{level}
											</button>
										)
									})}
								</div>
							</div>
						)}

						{reasoningMode !== "unsupported" && (
							<div className="flex flex-col gap-1.5 sm:col-span-2">
								<Label htmlFor="default-reasoning" className="text-xs text-muted-foreground">
									Default reasoning
								</Label>
								<Select
									value={defaultReasoningSelection || "__unset__"}
									onValueChange={(value) => {
										if (value == null || value === "__unset__") {
											setDefaultReasoningSelection("")
											return
										}
										setDefaultReasoningSelection(value)
									}}
									disabled={saving}
								>
									<SelectTrigger id="default-reasoning" className="h-9">
										<SelectValue placeholder="Model default" />
									</SelectTrigger>
									<SelectContent>
										<SelectGroup>
											<SelectItem value="__unset__">Unset</SelectItem>
											{effortOptions.map((option) => (
												<SelectItem key={option} value={option}>
													{option}
												</SelectItem>
											))}
										</SelectGroup>
									</SelectContent>
								</Select>
							</div>
						)}
					</div>

					<div className="rounded-lg border border-border/50">
						<button
							type="button"
							className="flex w-full items-center justify-between gap-2 px-3 py-2.5 text-left"
							onClick={() => setAdvancedOpen((v) => !v)}
						>
							<div className="min-w-0">
								<p className="text-sm font-medium tracking-tight">Advanced</p>
								{!advancedOpen && advancedSummary ? (
									<p className="truncate text-xs text-muted-foreground">{advancedSummary}</p>
								) : (
									<p className="text-xs text-muted-foreground">
										Context, sampling, modalities, headers, options, request body, effort encodings
									</p>
								)}
							</div>
							<ChevronDownIcon
								className={cn(
									"size-3.5 shrink-0 stroke-[1.5] text-muted-foreground transition-transform",
									advancedOpen && "rotate-180",
								)}
							/>
						</button>

						{advancedOpen && (
							<div className="grid gap-3 border-t border-border/40 px-3 py-3 sm:grid-cols-2">
								<Field
									label="Context window"
									value={contextWindow}
									onChange={setContextWindow}
									placeholder="e.g. 1000000"
									disabled={saving}
									hint="Usable context for this model (tokens). Stored as a percentage of model capacity; occupancy and auto-compact follow it."
								/>
								<Field
									label="Max tokens"
									value={maxTokens}
									onChange={setMaxTokens}
									placeholder="optional"
									disabled={saving}
								/>
								<Field
									label="Temperature"
									value={temperature}
									onChange={setTemperature}
									placeholder="optional"
									disabled={saving}
								/>
								<Field
									label="Top P"
									value={topP}
									onChange={setTopP}
									placeholder="optional"
									disabled={saving}
								/>
								<Field
									label="Top K"
									value={topK}
									onChange={setTopK}
									placeholder="optional"
									disabled={saving}
								/>
								<div className="flex flex-col gap-1.5 sm:col-span-2">
									<Label className="text-xs text-muted-foreground">Input modalities</Label>
									<div className="flex flex-wrap gap-1.5">
										{MODALITY_OPTIONS.map((mod) => {
											const active = modalities.includes(mod)
											return (
												<button
													key={mod}
													type="button"
													disabled={saving}
													onClick={() => toggleModality(mod)}
													className={cn(
														"h-7 rounded-md px-2.5 text-xs transition-colors",
														active
															? "bg-muted font-medium text-foreground"
															: "bg-transparent text-muted-foreground hover:bg-muted/60",
													)}
												>
													{mod}
												</button>
											)
										})}
									</div>
								</div>

								<div className="flex flex-col gap-2 sm:col-span-2">
									<Label className="text-xs text-muted-foreground">Request headers</Label>
									{headers.length === 0 ? (
										<p className="text-[11px] text-muted-foreground">
											No custom headers. Merged into HTTP requests for this model.
										</p>
									) : (
										headers.map((header, index) => (
											<div key={index} className="flex items-center gap-2">
												<Input
													placeholder="Header-Name"
													value={header.key}
													onChange={(e) => updateHeader(index, "key", e.target.value)}
													disabled={saving}
													className="h-9 flex-1 font-mono text-[13px]"
												/>
												<Input
													placeholder="value"
													value={header.value}
													onChange={(e) => updateHeader(index, "value", e.target.value)}
													disabled={saving}
													className="h-9 flex-1 font-mono text-[13px]"
												/>
												<Button
													variant="ghost"
													size="sm"
													onClick={() => removeHeader(index)}
													disabled={saving}
													aria-label="Remove header"
												>
													<Trash2Icon className="size-3.5 stroke-[1.5]" />
												</Button>
											</div>
										))
									)}
									<Button
										variant="ghost"
										size="sm"
										className="self-start text-[13px]"
										onClick={addHeader}
										disabled={saving}
									>
										<PlusIcon className="size-3.5 stroke-[1.5]" />
										Add header
									</Button>
								</div>

								<div className="flex flex-col gap-1.5 sm:col-span-2">
									<Label htmlFor="model-options-body" className="text-xs text-muted-foreground">
										Options (JSON)
									</Label>
									<Textarea
										id="model-options-body"
										value={optionsBody}
										onChange={(e) => setOptionsBody(e.target.value)}
										placeholder='{"thinking":{"budget":4096}}'
										disabled={saving}
										className="min-h-24 font-mono text-[12px]"
									/>
									<p className="text-[11px] text-muted-foreground">
										Merged before request body. Use for SDK/provider option bags.
									</p>
								</div>

								<div className="flex flex-col gap-1.5 sm:col-span-2">
									<Label htmlFor="model-request-body" className="text-xs text-muted-foreground">
										Extra request body (JSON)
									</Label>
									<Textarea
										id="model-request-body"
										value={requestBody}
										onChange={(e) => setRequestBody(e.target.value)}
										placeholder={'{\n  "key": "value"\n}'}
										disabled={saving}
										className="min-h-28 font-mono text-[12px]"
									/>
									<p className="text-[11px] text-muted-foreground">
										Merged into the provider request body for this model. Leave empty to clear.
									</p>
								</div>

								{effortOptions.length > 0 && (
									<div className="flex flex-col gap-3 sm:col-span-2">
										<div>
											<Label className="text-xs text-muted-foreground">Effort encodings</Label>
											<p className="text-[11px] text-muted-foreground">
												Optional per-selection overrides keyed as catalog variants (`off` / `on` /
												levels). Leave empty to use built-in adapter thinking/effort fields.
											</p>
										</div>
										{effortOptions.map((selection) => {
											const encoding = effortEncodings[selection] ?? emptyEffortEncoding()
											return (
												<div
													key={selection}
													className="grid gap-2 rounded-md border border-border/40 p-3"
												>
													<p className="text-xs font-medium tracking-tight">{selection}</p>
													<Field
														label="Request model override"
														value={encoding.requestModel}
														onChange={(value) =>
															updateEffortEncoding(selection, { requestModel: value })
														}
														placeholder="optional wire model id"
														disabled={saving}
													/>
													<div className="flex flex-col gap-1.5">
														<Label className="text-xs text-muted-foreground">
															Request body (JSON)
														</Label>
														<Textarea
															value={encoding.requestBody}
															onChange={(e) =>
																updateEffortEncoding(selection, {
																	requestBody: e.target.value,
																})
															}
															placeholder='{"ext":{"effort":"H"}}'
															disabled={saving}
															className="min-h-20 font-mono text-[12px]"
														/>
													</div>
												</div>
											)
										})}
									</div>
								)}
							</div>
						)}
					</div>

					{error && <p className="text-sm text-destructive">{error}</p>}
				</div>

				<div className="flex items-center justify-end gap-2 border-t border-border/50 px-6 py-3">
					<Button variant="outline" size="sm" onClick={() => onOpenChange(false)} disabled={saving}>
						Cancel
					</Button>
					<Button size="sm" onClick={handleSave} disabled={saving}>
						{saving && <Spinner className="size-3.5" />}
						{saving ? "Saving…" : isCreate ? "Add" : "Save"}
					</Button>
				</div>
			</DialogContent>
		</Dialog>
	)
}

function Field({
	label,
	value,
	onChange,
	placeholder,
	disabled,
	hint,
}: {
	label: string
	value: string
	onChange: (v: string) => void
	placeholder?: string
	disabled?: boolean
	hint?: string
}) {
	return (
		<div className="flex flex-col gap-1.5">
			<Label className="text-xs text-muted-foreground">{label}</Label>
			<Input
				value={value}
				onChange={(e) => onChange(e.target.value)}
				placeholder={placeholder}
				disabled={disabled}
				className="h-9"
			/>
			{hint ? <p className="text-[11px] leading-snug text-muted-foreground/80">{hint}</p> : null}
		</div>
	)
}

/** Short label for list rows. */
export function formatWireApiShort(
	model: CatalogModelInfo,
	provider: CatalogProviderInfo,
): string {
	const api = effectiveWireApi(model, provider)
	switch (api) {
		case "anthropic_messages":
			return "Anthropic"
		case "openai_responses":
			return "Responses"
		case "openai_chat_completions":
			return "Chat Completions"
		default:
			return api
	}
}
