import {
	SearchableListPopover,
	SearchableListPopoverContent,
	SearchableListPopoverEmpty,
	SearchableListPopoverGroup,
	SearchableListPopoverList,
	SearchableListPopoverSearch,
	SearchableListPopoverTrigger,
	useSearchableListPopoverSearch,
} from "@devo/ui/components/searchable-list-popover"
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { Separator } from "@devo/ui/components/separator"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { useIsMobile } from "@devo/ui/hooks/use-mobile"
import { cn } from "@devo/ui/lib/utils"
import { useAtomValue } from "jotai"
import {
	ChevronDownIcon,
	GitBranchIcon,
	MonitorIcon,
	SparklesIcon,
} from "lucide-react"
import { useCallback, useMemo, useState } from "react"
import { messagesFamily } from "../../atoms/messages"
import type {
	CompactionConfig,
	ModelRef,
	ProvidersData,
	SdkAgent,
	SdkProvider,
	VcsData,
} from "../../hooks/use-devo-data"
import {
	getModelCurrentVariant,
	getModelVariants,
	modelAllowsDefaultVariant,
	parseModelRef,
} from "../../hooks/use-devo-data"
import {
	computeContextUsage,
	formatPercentage,
	type ModelLimitInfo,
	shortModelName,
} from "../../lib/session-metrics"
import { ProviderIcon } from "../settings/provider-icon"
import { ModelSelectorOptionRow } from "./model-selector-option-row"
import {
	ModelSelectorReasoningStrength,
	ModelSelectorReasoningStrengthMobileView,
} from "./model-selector-reasoning-strength"
import { ModelSelectorTriggerLabel } from "./model-selector-trigger-label"
import { getVariantTriggerLabel, resolveSelectedVariant } from "./model-selector-variant-label"

// ============================================================
// Shared toolbar trigger styles
// ============================================================

/** Base classes shared by ALL toolbar triggers (Popover + Select). */
const TOOLBAR_TRIGGER_BASE_CN =
	"flex h-7 items-center gap-1 rounded-md border-none bg-transparent px-2 text-xs shadow-none transition-colors"

/**
 * Classes for SelectTrigger overrides. Uses `!` modifier to beat the base
 * component's `py-2 pl-2.5 pr-2 dark:bg-input/30 dark:hover:bg-input/50`.
 */
const TOOLBAR_TRIGGER_CN =
	"h-7! gap-1 border-none bg-transparent! hover:bg-muted! px-2! py-0! text-xs shadow-none transition-colors"

// ============================================================
// Agent Selector
// ============================================================

interface AgentSelectorProps {
	agents: SdkAgent[]
	selectedAgent: string | null
	defaultAgent?: string
	onSelectAgent: (agentName: string) => void
	disabled?: boolean
}

export function AgentSelector({
	agents,
	selectedAgent,
	defaultAgent,
	onSelectAgent,
	disabled,
}: AgentSelectorProps) {
	if (agents.length === 0) return null

	// Resolve which agent to display. If the preferred name doesn't match any
	// available agent (e.g. stale session data, config reload), fall back to a
	// known-valid agent so the Radix Select always has a matching SelectItem.
	const preferred = selectedAgent ?? defaultAgent ?? agents[0]?.name ?? "build"
	const currentAgentObj =
		agents.find((a) => a.name === preferred) ??
		agents.find((a) => a.name === defaultAgent) ??
		agents[0]
	const currentAgent = currentAgentObj?.name ?? preferred

	return (
		<Select
			value={currentAgent}
			onValueChange={(v) => {
				if (v !== null) onSelectAgent(v)
			}}
			disabled={disabled}
		>
			<SelectTrigger className={TOOLBAR_TRIGGER_CN}>
				<span className="flex items-center gap-1.5">
					{currentAgentObj?.color && (
						<span
							className="inline-block size-2 rounded-full"
							style={{ backgroundColor: currentAgentObj.color }}
						/>
					)}
					<span className="capitalize">{currentAgent}</span>
				</span>
			</SelectTrigger>
			<SelectContent side="top" align="start" alignItemWithTrigger={false}>
				{agents.map((agent) => (
					<SelectItem key={agent.name} value={agent.name}>
						<div className="flex items-center gap-2">
							{agent.color && (
								<span
									className="inline-block size-2 rounded-full"
									style={{ backgroundColor: agent.color }}
								/>
							)}
							<span className="capitalize">{agent.name}</span>
						</div>
					</SelectItem>
				))}
			</SelectContent>
		</Select>
	)
}

// ============================================================
// Model Selector (Combobox-based with search)
// ============================================================

interface ModelOption {
	/** Composite value: "providerID/modelID" */
	value: string
	providerID: string
	modelID: string
	displayName: string
	providerName: string
	reasoning: boolean
}

function flattenModels(providers: SdkProvider[]): ModelOption[] {
	const models: ModelOption[] = []
	for (const provider of providers) {
		for (const [key, model] of Object.entries(provider.models)) {
			const modelInfo = model as { name: string; capabilities?: { reasoning?: boolean } }
			models.push({
				value: `${provider.id}/${key}`,
				providerID: provider.id,
				modelID: key,
				displayName: modelInfo.name,
				providerName: provider.name,
				reasoning: modelInfo.capabilities?.reasoning ?? false,
			})
		}
	}
	return models
}

function groupByProvider(models: ModelOption[]): Map<string, ModelOption[]> {
	const groups = new Map<string, ModelOption[]>()
	for (const model of models) {
		const existing = groups.get(model.providerName)
		if (existing) {
			existing.push(model)
		} else {
			groups.set(model.providerName, [model])
		}
	}
	return groups
}

interface ModelSelectorProps {
	providers: ProvidersData | null
	/** The resolved effective model (after agent/config/default resolution) */
	effectiveModel: ModelRef | null
	/** Whether the user has explicitly overridden the model */
	hasOverride: boolean
	onSelectModel: (model: ModelRef | null) => void
	variants?: string[]
	selectedVariant?: string | undefined
	currentVariant?: string | undefined
	allowDefaultVariant?: boolean
	onSelectVariant?: (variant: string | undefined) => void
	disabled?: boolean
}

export function ModelSelector({
	providers,
	effectiveModel,
	onSelectModel,
	variants = [],
	selectedVariant,
	currentVariant,
	allowDefaultVariant = true,
	onSelectVariant = () => undefined,
	disabled,
}: ModelSelectorProps) {
	const models = useMemo(() => (providers ? flattenModels(providers.providers) : []), [providers])

	const activeValue = effectiveModel
		? `${effectiveModel.providerID}/${effectiveModel.modelID}`
		: null

	const activeModel = useMemo(
		() => models.find((m) => m.value === activeValue) ?? null,
		[models, activeValue],
	)
	const hasVariants = variants.length > 0
	const resolvedVariant = useMemo(
		() => resolveSelectedVariant(variants, selectedVariant, currentVariant, allowDefaultVariant),
		[allowDefaultVariant, currentVariant, selectedVariant, variants],
	)
	const variantTriggerLabel = hasVariants ? getVariantTriggerLabel(resolvedVariant) : null
	const isMobile = useIsMobile()

	const [open, setOpen] = useState(false)
	const [mobileVariantView, setMobileVariantView] = useState(false)

	const handleOpenChange = useCallback((nextOpen: boolean) => {
		setOpen(nextOpen)
		if (!nextOpen) setMobileVariantView(false)
	}, [])

	const handleSelect = useCallback(
		(value: string) => {
			const ref = parseModelRef(value)
			if (ref) {
				onSelectModel(ref)
			}
			setMobileVariantView(false)
			setOpen(false)
		},
		[onSelectModel],
	)

	if (!providers || models.length === 0) {
		return (
			<div className="flex items-center gap-1.5 text-xs text-muted-foreground/50">
				<SparklesIcon className="size-3" />
				<span>No models</span>
			</div>
		)
	}

	return (
		<SearchableListPopover open={open} onOpenChange={handleOpenChange}>
			<SearchableListPopoverTrigger
				className={cn(
					TOOLBAR_TRIGGER_BASE_CN,
					"hover:bg-muted disabled:cursor-not-allowed disabled:opacity-50",
				)}
				disabled={disabled}
			>
				{activeModel ? (
					<ModelSelectorTriggerLabel
						displayName={activeModel.displayName}
						variantLabel={variantTriggerLabel}
					/>
				) : (
					<span className="text-muted-foreground">Select model...</span>
				)}
				<ChevronDownIcon className="size-4 shrink-0 text-muted-foreground pointer-events-none" />
			</SearchableListPopoverTrigger>
			<SearchableListPopoverContent side="top" align="start" width="w-56">
				{mobileVariantView && hasVariants ? (
					<ModelSelectorReasoningStrengthMobileView
						variants={variants}
						selectedVariant={resolvedVariant}
						allowDefaultVariant={allowDefaultVariant}
						onBack={() => setMobileVariantView(false)}
						onSelectVariant={onSelectVariant}
						onClose={() => setOpen(false)}
					/>
				) : (
					<>
						<SearchableListPopoverSearch placeholder="Search models..." />
						<ModelSelectorList
							models={models}
							activeValue={activeValue}
							onSelect={handleSelect}
						/>
						{hasVariants && (
							<ModelSelectorReasoningStrength
								variants={variants}
								selectedVariant={resolvedVariant}
								allowDefaultVariant={allowDefaultVariant}
								isMobile={isMobile}
								onOpenMobileView={() => setMobileVariantView(true)}
								onSelectVariant={onSelectVariant}
								onClose={() => setOpen(false)}
							/>
						)}
					</>
				)}
			</SearchableListPopoverContent>
		</SearchableListPopover>
	)
}

/** Inner list component — reads search from context */
function ModelSelectorList({
	models,
	activeValue,
	onSelect,
}: {
	models: ModelOption[]
	activeValue: string | null
	onSelect: (value: string) => void
}) {
	const search = useSearchableListPopoverSearch()

	const filteredModels = useMemo(() => {
		if (!search) return models
		const q = search.toLowerCase()
		return models.filter(
			(m) =>
				m.displayName.toLowerCase().includes(q) ||
				m.providerName.toLowerCase().includes(q) ||
				m.modelID.toLowerCase().includes(q),
		)
	}, [models, search])

	const grouped = useMemo(() => groupByProvider(filteredModels), [filteredModels])

	return (
		<SearchableListPopoverList>
			{filteredModels.length === 0 ? (
				<SearchableListPopoverEmpty>No models found</SearchableListPopoverEmpty>
			) : (
				<>
					{/* Provider-grouped models */}
					{Array.from(grouped.entries()).map(([providerName, providerModels]) => {
						// Get the provider ID from the first model in the group to look up the icon
						const providerId = providerModels[0]?.providerID
						const items = providerModels.map((model) => (
							<ModelSelectorOptionRow
								key={model.value}
								displayName={model.displayName}
								reasoning={model.reasoning}
								selected={model.value === activeValue}
								onSelect={() => onSelect(model.value)}
							/>
						))
						if (providerId === "session") {
							return <div key={providerName}>{items}</div>
						}
						return (
							<SearchableListPopoverGroup
								key={providerName}
								label={
									<>
										{providerId && <ProviderIcon id={providerId} name={providerName} size="xs" />}
										<span>{providerName}</span>
									</>
								}
							>
								{items}
							</SearchableListPopoverGroup>
						)
					})}
				</>
			)}
		</SearchableListPopoverList>
	)
}

// ============================================================
// Variant Selector
// ============================================================

interface VariantSelectorProps {
	/** Available variant names for the current model */
	variants: string[]
	/** Currently selected variant (undefined = model default) */
	selectedVariant: string | undefined
	onSelectVariant: (variant: string | undefined) => void
	allowDefaultVariant?: boolean
	disabled?: boolean
}

export function VariantSelector({
	variants,
	selectedVariant,
	onSelectVariant,
	allowDefaultVariant = true,
	disabled,
}: VariantSelectorProps) {
	// Base UI Select needs an explicit items map so SelectValue can resolve
	// labels before the popup is opened (items only mount inside the portal).
	const items = useMemo(() => {
		const map: Record<string, string> = allowDefaultVariant
			? { __default__: "Default variant" }
			: {}
		for (const v of variants) {
			map[v] = v.charAt(0).toUpperCase() + v.slice(1)
		}
		return map
	}, [allowDefaultVariant, variants])

	if (variants.length === 0) return null

	// "default" is a sentinel for "no variant override".
	// If the selected variant isn't in the available list (e.g. stale restore),
	// fall back to the default sentinel when allowed, otherwise use the first
	// concrete option so reasoning-effort controls never show "Default variant".
	const value =
		selectedVariant && variants.includes(selectedVariant)
			? selectedVariant
			: allowDefaultVariant
				? "__default__"
				: variants[0]

	return (
		<Select
			value={value}
			onValueChange={(v) =>
				onSelectVariant(allowDefaultVariant && v === "__default__" ? undefined : (v ?? undefined))
			}
			disabled={disabled}
			items={items}
		>
			<SelectTrigger className={TOOLBAR_TRIGGER_CN}>
				<SelectValue />
			</SelectTrigger>
			<SelectContent
				side="top"
				align="start"
				alignItemWithTrigger={false}
				className="min-w-[160px]"
			>
				{allowDefaultVariant && (
					<SelectItem value="__default__">
						<span className="text-muted-foreground">Default variant</span>
					</SelectItem>
				)}
				{variants.map((variant) => (
					<SelectItem key={variant} value={variant}>
						<span className="capitalize">{variant}</span>
					</SelectItem>
				))}
			</SelectContent>
		</Select>
	)
}

// ============================================================
// Combined Prompt Toolbar
// ============================================================

export interface PromptToolbarProps {
	/** Available agents from Devo */
	agents: SdkAgent[]
	/** Currently selected agent name */
	selectedAgent: string | null
	/** Default agent from config */
	defaultAgent?: string
	onSelectAgent: (agentName: string) => void

	/** Provider data for model selector */
	providers: ProvidersData | null
	/** The resolved effective model */
	effectiveModel: ModelRef | null
	/** Whether the user has explicitly overridden the model */
	hasModelOverride: boolean
	onSelectModel: (model: ModelRef | null) => void

	/** Currently selected variant */
	selectedVariant: string | undefined
	onSelectVariant: (variant: string | undefined) => void

	disabled?: boolean
}

/**
 * Combined toolbar with agent, model, and variant selectors.
 * Renders inside the PromptInputFooter > PromptInputTools slot.
 */
export function PromptToolbar({
	agents,
	selectedAgent,
	defaultAgent,
	onSelectAgent,
	providers,
	effectiveModel,
	hasModelOverride,
	onSelectModel,
	selectedVariant,
	onSelectVariant,
	disabled,
}: PromptToolbarProps) {
	// Compute variants for the current effective model
	const variants = useMemo(() => {
		if (!effectiveModel || !providers) return []
		return getModelVariants(effectiveModel.providerID, effectiveModel.modelID, providers.providers)
	}, [effectiveModel, providers])
	const currentVariant = useMemo(() => {
		if (!effectiveModel || !providers) return undefined
		return getModelCurrentVariant(
			effectiveModel.providerID,
			effectiveModel.modelID,
			providers.providers,
		)
	}, [effectiveModel, providers])
	const allowDefaultVariant = useMemo(() => {
		if (!effectiveModel || !providers) return true
		return modelAllowsDefaultVariant(
			effectiveModel.providerID,
			effectiveModel.modelID,
			providers.providers,
		)
	}, [effectiveModel, providers])

	const hasAgents = agents.length > 0

	return (
		<div className="flex min-w-0 flex-wrap items-center gap-0.5">
			{hasAgents && (
				<AgentSelector
					agents={agents}
					selectedAgent={selectedAgent}
					defaultAgent={defaultAgent}
					onSelectAgent={onSelectAgent}
					disabled={disabled}
				/>
			)}

			{hasAgents && <Separator orientation="vertical" className="mx-0.5 my-2 self-stretch" />}

			<ModelSelector
				providers={providers}
				effectiveModel={effectiveModel}
				hasOverride={hasModelOverride}
				onSelectModel={onSelectModel}
				variants={variants}
				selectedVariant={selectedVariant}
				currentVariant={currentVariant}
				allowDefaultVariant={allowDefaultVariant}
				onSelectVariant={onSelectVariant}
				disabled={disabled}
			/>
		</div>
	)
}

// ============================================================
// Status Bar (below the input card)
// ============================================================

interface StatusBarProps {
	vcs: VcsData | null
	isConnected: boolean
	/** Whether the session is currently running */
	isWorking?: boolean
	/** Number of Escape presses toward abort (0 = none, 1 = first press) */
	interruptCount?: number
	/** Optional slot to replace the default branch display (e.g. interactive BranchPicker) */
	branchSlot?: React.ReactNode
	/** Optional extra slot rendered on the left side (e.g. worktree toggle) */
	extraSlot?: React.ReactNode
	/** Session ID for context usage computation */
	sessionId?: string
	/** Provider data for context limit lookup */
	providers?: ProvidersData | null
	/** Compaction config from Devo for accurate threshold calculation */
	compaction?: CompactionConfig
}

export function StatusBar({
	vcs,
	isConnected,
	isWorking,
	interruptCount,
	branchSlot,
	extraSlot,
	sessionId,
	providers,
	compaction,
}: StatusBarProps) {
	return (
		<div className="flex min-w-0 items-center gap-3 overflow-hidden px-2 pt-2 text-[11px] text-muted-foreground/60">
			{/* Left side — environment + connection + interrupt hint */}
			<div className="flex shrink-0 items-center gap-3">
				{extraSlot ?? (
					<div className="flex items-center gap-1">
						<MonitorIcon className="size-3" />
						<span>Local</span>
					</div>
				)}

				{!isConnected && (
					<div className="flex items-center gap-1 text-yellow-500/70">
						<span className="inline-block size-1.5 rounded-full bg-yellow-500/70" />
						<span>Disconnected</span>
					</div>
				)}

				{/* Escape-to-abort hint — shown when session is working */}
				{isConnected && isWorking && (
					<div
						className={`flex items-center gap-1 transition-colors ${interruptCount && interruptCount > 0 ? "text-orange-400" : ""}`}
					>
						<kbd className="rounded border border-border px-1 py-0.5 font-mono text-[10px] leading-none">
							esc
						</kbd>
						<span>
							{interruptCount && interruptCount > 0 ? "press again to stop" : "interrupt"}
						</span>
					</div>
				)}
			</div>

			{/* Right side — context usage + git branch */}
			<div className="ml-auto flex min-w-0 items-center gap-3 overflow-hidden">
				{/* Context window usage */}
				{sessionId && (
					<ContextUsageIndicator
						sessionId={sessionId}
						providers={providers}
						compaction={compaction}
					/>
				)}

				{/* Git branch — interactive picker or read-only display */}
				{branchSlot
					? branchSlot
					: vcs?.branch && (
							<div className="flex items-center gap-1">
								<GitBranchIcon className="size-3" />
								<span className="max-w-[140px] truncate">{vcs.branch}</span>
							</div>
						)}
			</div>
		</div>
	)
}

// ============================================================
// Context window usage indicator (for StatusBar)
// ============================================================

/**
 * Compact context usage indicator: circular progress + percentage.
 * Reads messages from the Jotai atom and computes context window usage
 * against the model's context limit from provider data.
 *
 * Renders nothing when there are no assistant messages with token data,
 * or when provider data is unavailable for the current model.
 */
function ContextUsageIndicator({
	sessionId,
	providers,
	compaction,
}: {
	sessionId: string
	providers?: ProvidersData | null
	compaction?: CompactionConfig
}) {
	const messages = useAtomValue(messagesFamily(sessionId))

	const getModelLimit = useCallback(
		(providerID: string, modelID: string): ModelLimitInfo | undefined => {
			if (!providers?.providers) return undefined
			for (const provider of providers.providers) {
				if (provider.id !== providerID) continue
				const model = provider.models[modelID]
				if (model?.limit?.context) return model.limit
			}
			return undefined
		},
		[providers],
	)

	const compactionOptions = useMemo(
		() => (compaction ? { auto: compaction.auto, reserved: compaction.reserved } : undefined),
		[compaction],
	)

	const usage = useMemo(
		() => computeContextUsage(messages, getModelLimit, compactionOptions),
		[messages, getModelLimit, compactionOptions],
	)

	if (!usage) return null

	const pct = usage.percentage
	const color = pct >= 90 ? "text-red-400" : pct >= 70 ? "text-yellow-400" : ""

	const compPct = usage.compactionPercentage
	const compColor =
		compPct != null && compPct >= 100
			? "text-red-400"
			: compPct != null && compPct >= 80
				? "text-yellow-400"
				: "text-background/60"

	return (
		<Tooltip>
			<TooltipTrigger
				render={
					<span
						className={cn(
							"inline-flex items-center gap-1 tabular-nums transition-colors hover:text-foreground",
							color,
						)}
					/>
				}
			>
				<ContextCircle percentage={pct} size={12} strokeWidth={1.5} />
				<span>{formatPercentage(pct)}</span>
			</TooltipTrigger>
			<TooltipContent side="top" align="end">
				<div className="space-y-1.5 text-xs">
					<p className="font-medium">Context Window</p>
					<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-background/60">
						<span>Usage</span>
						<span className="text-right tabular-nums">{formatPercentage(pct)}</span>
						<span>Tokens</span>
						<span className="text-right tabular-nums">
							{usage.lastMessageTokens.toLocaleString()}
						</span>
						<span>Limit</span>
						<span className="text-right tabular-nums">{usage.contextLimit.toLocaleString()}</span>
						<span>Model</span>
						<span className="text-right">{shortModelName(usage.modelID)}</span>
					</div>
					{usage.compactionThreshold != null && compPct != null && (
						<div className="border-t border-background/15 pt-1">
							<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-background/60">
								<span>Compaction</span>
								<span className={cn("text-right tabular-nums", compColor)}>
									{compPct >= 100 ? "now" : `at ${usage.compactionThreshold.toLocaleString()}`}
								</span>
								<span>Remaining</span>
								<span className={cn("text-right tabular-nums", compColor)}>
									{compPct >= 100
										? "overflowed"
										: `${(usage.compactionThreshold - usage.lastMessageTokens).toLocaleString()} tokens`}
								</span>
							</div>
						</div>
					)}
				</div>
			</TooltipContent>
		</Tooltip>
	)
}

// ============================================================
// SVG circular progress
// ============================================================

function ContextCircle({
	percentage,
	size = 12,
	strokeWidth = 1.5,
}: {
	percentage: number
	size?: number
	strokeWidth?: number
}) {
	const radius = (size - strokeWidth) / 2
	const circumference = 2 * Math.PI * radius
	const offset = circumference - (Math.min(percentage, 100) / 100) * circumference

	const strokeColor =
		percentage >= 90 ? "stroke-red-400" : percentage >= 70 ? "stroke-yellow-400" : "stroke-current"

	return (
		<svg
			width={size}
			height={size}
			viewBox={`0 0 ${size} ${size}`}
			className="shrink-0"
			aria-hidden="true"
		>
			<circle
				cx={size / 2}
				cy={size / 2}
				r={radius}
				fill="none"
				className="stroke-muted-foreground/15"
				strokeWidth={strokeWidth}
			/>
			<circle
				cx={size / 2}
				cy={size / 2}
				r={radius}
				fill="none"
				className={strokeColor}
				strokeWidth={strokeWidth}
				strokeDasharray={circumference}
				strokeDashoffset={offset}
				strokeLinecap="round"
				transform={`rotate(-90 ${size / 2} ${size / 2})`}
			/>
		</svg>
	)
}
