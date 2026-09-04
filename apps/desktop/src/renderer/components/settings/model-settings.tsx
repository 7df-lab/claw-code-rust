/**
 * Settings → Models.
 *
 * Lists models under connected providers. Enable toggles write model.enabled
 * overlays; clicking a row opens the model editor (wire API is per-model).
 */

import type { CatalogModelInfo, CatalogProviderInfo } from "@devo-ai/sdk/v2/client"
import { Button } from "@devo/ui/components/button"
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@devo/ui/components/empty"
import { Input } from "@devo/ui/components/input"
import { Skeleton } from "@devo/ui/components/skeleton"
import { Switch } from "@devo/ui/components/switch"
import {
	AlertCircleIcon,
	PlusIcon,
	PlugZapIcon,
	RefreshCwIcon,
	SearchIcon,
	Settings2Icon,
} from "lucide-react"
import { useCallback, useMemo, useRef, useState } from "react"
import { useNavigate } from "@tanstack/react-router"
import {
	effectiveContextWindowTokens,
	formatContextWindowLabel,
} from "../../lib/providers"
import { invalidateProviderDependentQueries } from "../../lib/invalidate-provider-queries"
import { useProviderCatalog } from "../../hooks/use-devo-data"
import { getBaseClient } from "../../services/connection-manager"
import { formatWireApiShort, ModelEditDialog } from "./model-edit-dialog"
import { ProviderIcon } from "./provider-icon"
import { SettingsHeader } from "./settings-header"
import { SettingsSection } from "./settings-section"
import {
	settingsBannerClass,
	settingsPageClass,
	settingsPanelClass,
} from "./settings-surface"

interface ModelRow {
	providerId: string
	providerName: string
	provider: CatalogProviderInfo
	modelId: string
	model: CatalogModelInfo
	/** Explicit enabled flag from catalog; unset means default-on (true). */
	enabled: boolean
	/** Whether the catalog explicitly set enabled (vs inheriting default). */
	enabledExplicit: boolean
	qualifiedId: string
}

/** Keep known model ids in their first-seen order; append only new ids. */
function stabilizeModelOrder(previous: string[] | undefined, ids: string[]): string[] {
	if (!previous?.length) return ids
	const idSet = new Set(ids)
	const ordered = previous.filter((id) => idSet.has(id))
	for (const id of ids) {
		if (!ordered.includes(id)) ordered.push(id)
	}
	return ordered
}

export function ModelSettings() {
	const { data: catalog, loading, error, reload } = useProviderCatalog()
	const [search, setSearch] = useState("")
	const [togglingModels, setTogglingModels] = useState<Set<string>>(new Set())
	const [editing, setEditing] = useState<ModelRow | null>(null)
	const [addingTo, setAddingTo] = useState<CatalogProviderInfo | null>(null)
	const navigate = useNavigate()
	/** Per-provider model id order so enable/disable does not reshuffle the list. */
	const modelOrderRef = useRef<Map<string, string[]>>(new Map())

	const providerById = useMemo(() => {
		if (!catalog) return new Map<string, CatalogProviderInfo>()
		const map = new Map<string, CatalogProviderInfo>()
		for (const provider of catalog.providers) {
			if (!catalog.connectedIds.has(provider.id)) continue
			const models: Record<string, CatalogModelInfo> = {
				...(provider.models ?? {}),
				...(catalog.connectionModels[provider.id] ?? {}),
			}
			const orderedIds = stabilizeModelOrder(
				modelOrderRef.current.get(provider.id),
				Object.keys(models),
			)
			modelOrderRef.current.set(provider.id, orderedIds)
			const orderedModels: Record<string, CatalogModelInfo> = {}
			for (const modelId of orderedIds) {
				orderedModels[modelId] = models[modelId]
			}
			map.set(provider.id, { ...provider, models: orderedModels })
		}
		return map
	}, [catalog])

	const rows = useMemo((): ModelRow[] => {
		const result: ModelRow[] = []
		for (const provider of providerById.values()) {
			for (const [modelId, model] of Object.entries(provider.models ?? {})) {
				result.push({
					providerId: provider.id,
					providerName: provider.name,
					provider,
					modelId,
					model,
					enabled: model.enabled !== false,
					enabledExplicit: typeof model.enabled === "boolean",
					qualifiedId: `${provider.id}/${modelId}`,
				})
			}
		}
		return result
	}, [providerById])

	const filteredRows = useMemo(() => {
		if (!search.trim()) return rows
		const lower = search.toLowerCase()
		return rows.filter(
			(r) =>
				r.modelId.toLowerCase().includes(lower) ||
				(r.model.name ?? "").toLowerCase().includes(lower) ||
				r.providerName.toLowerCase().includes(lower),
		)
	}, [rows, search])

	const groups = useMemo(() => {
		const map = new Map<string, { provider: CatalogProviderInfo; models: ModelRow[] }>()
		const searching = search.trim().length > 0

		if (!searching) {
			for (const provider of providerById.values()) {
				map.set(provider.id, { provider, models: [] })
			}
		}

		for (const row of filteredRows) {
			const existing = map.get(row.providerId)
			if (existing) {
				existing.models.push(row)
			} else {
				map.set(row.providerId, { provider: row.provider, models: [row] })
			}
		}
		return [...map.values()]
	}, [filteredRows, providerById, search])

	const handleToggleModel = useCallback(
		async (providerId: string, modelId: string, enabled: boolean) => {
			const qualifiedId = `${providerId}/${modelId}`
			setTogglingModels((prev) => new Set(prev).add(qualifiedId))
			try {
				const client = getBaseClient()
				if (!client) return
				const provider = catalog?.providers.find((p) => p.id === providerId)
				if (!provider || !catalog) return
				const updatedModels = {
					...(provider.models ?? {}),
					...(catalog.connectionModels[providerId] ?? {}),
				}
				updatedModels[modelId] = {
					...updatedModels[modelId],
					enabled,
				}
				await client.provider.upsert({
					provider: { ...provider, models: updatedModels },
				})
				invalidateProviderDependentQueries()
			} finally {
				setTogglingModels((prev) => {
					const next = new Set(prev)
					next.delete(qualifiedId)
					return next
				})
			}
		},
		[catalog],
	)

	const handleSaved = useCallback(() => {
		invalidateProviderDependentQueries()
		reload()
	}, [reload])

	if (loading) {
		return (
			<div className={settingsPageClass}>
				<SettingsHeader title="Models" />
				<div className="flex flex-col gap-4">
					<Skeleton className="h-9 w-full rounded-lg" />
					<Skeleton className="h-28 w-full rounded-xl" />
					<Skeleton className="h-28 w-full rounded-xl" />
				</div>
			</div>
		)
	}

	if (error) {
		return (
			<div className={settingsPageClass}>
				<SettingsHeader title="Models" />
				<div className={settingsBannerClass}>
					<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
					<span>Failed to load models: {error}</span>
					<Button variant="outline" size="sm" className="ml-auto" onClick={reload}>
						<RefreshCwIcon data-icon="inline-start" />
						Retry
					</Button>
				</div>
			</div>
		)
	}

	const hasConnectedProviders = catalog && catalog.connectedIds.size > 0

	return (
		<div className={settingsPageClass}>
			<SettingsHeader
				title="Models"
				description="Enable models and edit invocation method, context, and sampling. Wire API is set per model."
			/>

			{!hasConnectedProviders ? (
				<Empty className={settingsPanelClass}>
					<EmptyHeader>
						<EmptyMedia variant="icon">
							<PlugZapIcon aria-hidden="true" />
						</EmptyMedia>
						<EmptyTitle className="text-lg font-medium tracking-tight">
							No connected providers
						</EmptyTitle>
						<EmptyDescription className="text-sm leading-6">
							Connect a provider first to see and manage its models.
						</EmptyDescription>
					</EmptyHeader>
					<EmptyContent>
						<Button
							variant="outline"
							size="sm"
							onClick={() => navigate({ to: "/settings/providers" })}
						>
							<PlugZapIcon data-icon="inline-start" />
							Go to Providers
						</Button>
					</EmptyContent>
				</Empty>
			) : (
				<>
					<div className="relative">
						<SearchIcon className="absolute left-3 top-1/2 size-3.5 -translate-y-1/2 stroke-[1.5] text-muted-foreground" />
						<Input
							placeholder="Search models…"
							value={search}
							onChange={(e) => setSearch(e.target.value)}
							className="h-9 pl-9 shadow-none"
						/>
					</div>

					{groups.length === 0 && search.trim() ? (
						<p className="py-6 text-center text-sm text-muted-foreground">
							No models match "{search}"
						</p>
					) : (
						groups.map(({ provider, models }) => (
							<SettingsSection key={provider.id}>
								<div className="flex items-center gap-2.5 border-b border-border/40 px-4 py-2.5">
									<ProviderIcon id={provider.id} name={provider.name} size="xs" />
									<span className="min-w-0 flex-1 truncate text-sm font-medium tracking-tight">
										{provider.name}
									</span>
									<span className="text-xs text-muted-foreground">
										{models.filter((m) => m.enabled).length}/{models.length} enabled
									</span>
									<Button
										variant="ghost"
										size="sm"
										className="shrink-0 text-muted-foreground"
										onClick={() => setAddingTo(provider)}
									>
										<PlusIcon className="size-3.5 stroke-[1.5]" />
										Add model
									</Button>
								</div>
								{models.length === 0 ? (
									<p className="px-4 py-5 text-center text-sm text-muted-foreground">
										No models yet. Add one to get started.
									</p>
								) : (
									models.map((row) => (
										<ModelRowView
											key={row.qualifiedId}
											row={row}
											toggling={togglingModels.has(row.qualifiedId)}
											onToggle={handleToggleModel}
											onEdit={() => setEditing(row)}
										/>
									))
								)}
							</SettingsSection>
						))
					)}
				</>
			)}

			{editing && (
				<ModelEditDialog
					key={editing.qualifiedId}
					provider={editing.provider}
					modelId={editing.modelId}
					model={editing.model}
					mode="edit"
					open={!!editing}
					onOpenChange={(open) => {
						if (!open) setEditing(null)
					}}
					onSaved={handleSaved}
				/>
			)}

			{addingTo && (
				<ModelEditDialog
					key={`add-${addingTo.id}`}
					provider={addingTo}
					mode="create"
					open={!!addingTo}
					onOpenChange={(open) => {
						if (!open) setAddingTo(null)
					}}
					onSaved={handleSaved}
				/>
			)}
		</div>
	)
}

function ModelRowView({
	row,
	toggling,
	onToggle,
	onEdit,
}: {
	row: ModelRow
	toggling: boolean
	onToggle: (providerId: string, modelId: string, enabled: boolean) => void
	onEdit: () => void
}) {
	const wire = formatWireApiShort(row.model, row.provider)
	const contextLabel = formatContextWindowLabel(effectiveContextWindowTokens(row.model))

	return (
		<div className="flex items-center gap-3 px-4 py-3">
			<button
				type="button"
				className="min-w-0 flex-1 text-left"
				onClick={onEdit}
			>
				<p className="truncate text-sm tracking-tight">{row.model.name ?? row.modelId}</p>
				<p className="mt-0.5 truncate text-xs text-muted-foreground">
					<span className="font-mono">{row.modelId}</span>
					<span className="mx-1.5 text-border">·</span>
					{wire}
					{contextLabel && (
						<>
							<span className="mx-1.5 text-border">·</span>
							{contextLabel}
						</>
					)}
				</p>
			</button>
			<Button
				variant="ghost"
				size="sm"
				className="shrink-0 text-muted-foreground"
				onClick={onEdit}
				aria-label={`Edit ${row.modelId}`}
			>
				<Settings2Icon className="size-3.5 stroke-[1.5]" />
			</Button>
			<Switch
				checked={row.enabled}
				disabled={toggling}
				onCheckedChange={(checked) => onToggle(row.providerId, row.modelId, checked)}
				aria-label={row.enabled ? "Disable model" : "Enable model"}
			/>
		</div>
	)
}
