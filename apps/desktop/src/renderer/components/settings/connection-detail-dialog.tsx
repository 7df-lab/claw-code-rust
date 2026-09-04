/**
 * Dialog showing a connected provider's model list.
 *
 * Allows adding models, removing individual models, and triggering
 * model discovery from the provider's /models endpoint.
 */

import type { CatalogModelInfo, CatalogProviderInfo } from "@devo-ai/sdk/v2/client"
import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { Spinner } from "@devo/ui/components/spinner"
import { PlusIcon, RefreshCwIcon, SearchIcon, Trash2Icon } from "lucide-react"
import { useCallback, useMemo, useState } from "react"
import { getBaseClient } from "../../services/connection-manager"
import {
	effectiveContextWindowTokens,
	formatContextWindowLabel,
} from "../../lib/providers"
import { ModelEditDialog } from "./model-edit-dialog"
import { ProviderIcon } from "./provider-icon"

interface ConnectionDetailDialogProps {
	provider: CatalogProviderInfo
	connectionModels: Record<string, CatalogModelInfo>
	open: boolean
	onOpenChange: (open: boolean) => void
	onChanged: () => void
}

export function ConnectionDetailDialog({
	provider,
	connectionModels,
	open,
	onOpenChange,
	onChanged,
}: ConnectionDetailDialogProps) {
	const [search, setSearch] = useState("")
	const [discovering, setDiscovering] = useState(false)
	const [removingModel, setRemovingModel] = useState<string | null>(null)
	const [addingModel, setAddingModel] = useState(false)

	const models = useMemo(() => {
		const entries = Object.entries(connectionModels)
		if (!search.trim()) return entries
		const lower = search.toLowerCase()
		return entries.filter(
			([id, m]) =>
				id.toLowerCase().includes(lower) ||
				(m.name ?? "").toLowerCase().includes(lower),
		)
	}, [connectionModels, search])

	const handleDiscover = useCallback(async () => {
		setDiscovering(true)
		try {
			const client = getBaseClient()
			if (!client) return
			await client.provider.discover({
				providerId: provider.id,
				forceRefresh: true,
			})
			onChanged()
		} finally {
			setDiscovering(false)
		}
	}, [provider.id, onChanged])

	const handleRemoveModel = useCallback(
		async (modelId: string) => {
			setRemovingModel(modelId)
			try {
				const client = getBaseClient()
				if (!client) return
				await client.provider.modelRemove({
					providerId: provider.id,
					modelId,
				})
				onChanged()
			} finally {
				setRemovingModel(null)
			}
		},
		[provider.id, onChanged],
	)

	return (
		<>
			<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="max-w-lg max-h-[80vh] overflow-y-auto">
				<DialogHeader>
					<div className="flex items-center gap-3">
						<ProviderIcon id={provider.id} name={provider.name} size="xs" />
						<DialogTitle>{provider.name}</DialogTitle>
						<Badge variant="secondary">Connected</Badge>
					</div>
					<DialogDescription>
						{Object.keys(connectionModels).length} model
						{Object.keys(connectionModels).length !== 1 ? "s" : ""} in this Connection.
						{provider.baseUrl ? ` Endpoint: ${provider.baseUrl}` : ""}
					</DialogDescription>
				</DialogHeader>

				{/* Toolbar */}
				<div className="flex items-center gap-2">
					<div className="relative flex-1">
						<SearchIcon className="absolute left-3 top-1/2 size-3.5 -translate-y-1/2 stroke-[1.5] text-muted-foreground" />
						<Input
							placeholder="Filter models…"
							value={search}
							onChange={(e) => setSearch(e.target.value)}
							className="h-8 pl-8 text-[13px]"
						/>
					</div>
					<Button
						variant="outline"
						size="sm"
						onClick={handleDiscover}
						disabled={discovering}
					>
						{discovering ? <Spinner className="size-3.5" /> : <RefreshCwIcon className="size-3.5 stroke-[1.5]" />}
						Discover
					</Button>
					<Button
						variant="outline"
						size="sm"
						onClick={() => setAddingModel(true)}
					>
						<PlusIcon className="size-3.5 stroke-[1.5]" />
						Add
					</Button>
				</div>

				{/* Model list */}
				<div className="divide-y divide-border/40 overflow-hidden rounded-xl border border-border/50">
					{models.length === 0 ? (
						<div className="px-4 py-6 text-center text-xs text-muted-foreground">
							{search.trim() ? `No models match "${search}"` : "No models in this Connection."}
						</div>
					) : (
						models.map(([modelId, model]) => {
							const contextLabel = formatContextWindowLabel(
								effectiveContextWindowTokens(model),
							)
							return (
							<div key={modelId} className="flex items-center gap-3 px-4 py-2.5">
								<div className="min-w-0 flex-1">
									<p className="truncate text-sm tracking-tight">{model.name ?? modelId}</p>
									<p className="truncate text-xs text-muted-foreground">
										{provider.id}/{modelId}
									</p>
								</div>
								{contextLabel && (
									<span className="shrink-0 text-[11px] text-muted-foreground">
										{contextLabel}
									</span>
								)}
								<Button
									variant="ghost"
									size="sm"
									className="text-muted-foreground hover:text-destructive"
									onClick={() => handleRemoveModel(modelId)}
									disabled={removingModel === modelId}
								>
									{removingModel === modelId ? (
										<Spinner className="size-3" />
									) : (
										<Trash2Icon className="size-3.5 stroke-[1.5]" />
									)}
								</Button>
							</div>
							)
						})
					)}
				</div>

				<DialogFooter>
					<Button variant="outline" onClick={() => onOpenChange(false)}>
						Done
					</Button>
				</DialogFooter>
			</DialogContent>
			</Dialog>

			{addingModel && (
				<ModelEditDialog
					key={`add-${provider.id}`}
					provider={provider}
					mode="create"
					open={addingModel}
					onOpenChange={(open) => { if (!open) setAddingModel(false) }}
					onSaved={onChanged}
				/>
			)}
		</>
	)
}
