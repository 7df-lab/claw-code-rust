/**
 * Settings → Providers.
 *
 * Shows connected providers (Connections) and built-in templates.
 * Uses the canonical `provider/list` catalog shape (L2-DES-MODEL-002).
 */

import type { CatalogProviderInfo } from "@devo-ai/sdk/v2/client"
import { Button } from "@devo/ui/components/button"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Skeleton } from "@devo/ui/components/skeleton"
import {
	AlertCircleIcon,
	MinusIcon,
	PlusIcon,
	RefreshCwIcon,
} from "lucide-react"
import { useNavigate } from "@tanstack/react-router"
import { useCallback, useMemo, useState } from "react"
import { useProviderCatalog } from "../../hooks/use-devo-data"
import { invalidateProviderDependentQueries } from "../../lib/invalidate-provider-queries"
import { POPULAR_PROVIDER_IDS } from "../../lib/providers"
import { getBaseClient } from "../../services/connection-manager"
import { ProviderIcon } from "./provider-icon"
import { SettingsHeader } from "./settings-header"
import { SettingsSection } from "./settings-section"
import { settingsBannerClass, settingsPageClass } from "./settings-surface"
import { TemplateConnectDialog } from "./template-connect-dialog"
import { CustomProviderDialog } from "./custom-provider-dialog"
import { ConnectionDetailDialog } from "./connection-detail-dialog"

// ============================================================
// Main component
// ============================================================

export function ProviderSettings() {
	const { data: catalog, loading, error, reload } = useProviderCatalog()
	const navigate = useNavigate()

	// Dialog state
	const [connectTemplate, setConnectTemplate] = useState<CatalogProviderInfo | null>(null)
	const [customDialogOpen, setCustomDialogOpen] = useState(false)
	const [connectionDetail, setConnectionDetail] = useState<CatalogProviderInfo | null>(null)
	const [disconnectTarget, setDisconnectTarget] = useState<CatalogProviderInfo | null>(null)
	const [disconnecting, setDisconnecting] = useState(false)
	const [showAllTemplates, setShowAllTemplates] = useState(false)

	// Split providers into connected vs templates
	const connected = useMemo(() => {
		if (!catalog) return []
		return catalog.providers.filter((p) => catalog.connectedIds.has(p.id))
	}, [catalog])

	const templates = useMemo(() => {
		if (!catalog) return []
		// Keep connected templates visible in Popular — no Connected badge here;
		// Connected section remains the place for manage/remove.
		const all = catalog.providers.filter((p) => catalog.templateIds.has(p.id))
		if (showAllTemplates) return all
		const popularSet = new Set<string>(POPULAR_PROVIDER_IDS)
		const popular = all.filter((p) => popularSet.has(p.id))
		return popular.length > 0 ? popular : all.slice(0, 8)
	}, [catalog, showAllTemplates])

	const hasMoreTemplates = useMemo(() => {
		if (!catalog || showAllTemplates) return false
		const allTemplateCount = catalog.providers.filter((p) =>
			catalog.templateIds.has(p.id),
		).length
		return allTemplateCount > templates.length
	}, [catalog, templates, showAllTemplates])

	const handleDisconnect = useCallback(async () => {
		if (!disconnectTarget) return
		setDisconnecting(true)
		try {
			const client = getBaseClient()
			if (!client) return
			await client.provider.disconnect({ providerId: disconnectTarget.id })
			invalidateProviderDependentQueries()
		} finally {
			setDisconnecting(false)
			setDisconnectTarget(null)
		}
	}, [disconnectTarget])

	const handleSaved = useCallback(() => {
		invalidateProviderDependentQueries()
		reload()
	}, [reload])

	/** After connecting a new provider, take the user to Models to review/enable them. */
	const handleConnected = useCallback(() => {
		handleSaved()
		void navigate({ to: "/settings/models" })
	}, [handleSaved, navigate])

	if (loading) {
		return <ProviderSettingsLoading />
	}

	if (error) {
		return (
			<div className={settingsPageClass}>
				<ProviderSettingsPageHeader onAddCustom={() => setCustomDialogOpen(true)} />
				<div className={settingsBannerClass}>
					<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
					<span>Failed to load providers: {error}</span>
					<Button variant="outline" size="sm" className="ml-auto" onClick={reload}>
						<RefreshCwIcon data-icon="inline-start" />
						Retry
					</Button>
				</div>
			</div>
		)
	}

	return (
		<div className={settingsPageClass}>
			<ProviderSettingsPageHeader onAddCustom={() => setCustomDialogOpen(true)} />

			{/* Connected providers */}
			<SettingsSection title="Connected">
				{connected.length === 0 ? (
					<div className="px-4 py-6 text-center text-sm text-muted-foreground">
						No connected providers. Connect a template below or add a custom provider.
					</div>
				) : (
					connected.map((provider) => (
						<ConnectedProviderRow
							key={provider.id}
							provider={provider}
							modelCount={Object.keys(catalog?.connectionModels[provider.id] ?? provider.models ?? {}).length}
							onOpen={() => setConnectionDetail(provider)}
							onDisconnect={() => setDisconnectTarget(provider)}
						/>
					))
				)}
			</SettingsSection>

			{/* Template providers */}
			{templates.length > 0 && (
				<SettingsSection
					title="Popular Providers"
					action={
						hasMoreTemplates ? (
							<Button
								variant="ghost"
								size="sm"
								className="text-[13px]"
								onClick={() => setShowAllTemplates(true)}
							>
								See more providers
							</Button>
						) : undefined
					}
				>
					{templates.map((provider) => (
						<TemplateProviderRow
							key={provider.id}
							provider={provider}
							onConnect={() => {
								if (catalog?.connectedIds.has(provider.id)) {
									setConnectionDetail(provider)
									return
								}
								setConnectTemplate(provider)
							}}
						/>
					))}
				</SettingsSection>
			)}

			{/* Footer: add custom */}
			<div className="flex items-center justify-center gap-4 px-1">
				<Button
					variant="outline"
					size="sm"
					onClick={() => setCustomDialogOpen(true)}
				>
					<PlusIcon data-icon="inline-start" />
					Add custom provider
				</Button>
			</div>

			{/* Dialogs */}
			{connectTemplate && (
				<TemplateConnectDialog
					provider={connectTemplate}
					open={!!connectTemplate}
					onOpenChange={(open) => { if (!open) setConnectTemplate(null) }}
					onConnected={handleConnected}
				/>
			)}

			{customDialogOpen && (
				<CustomProviderDialog
					open={customDialogOpen}
					onOpenChange={setCustomDialogOpen}
					onSaved={handleConnected}
				/>
			)}

			{connectionDetail && catalog && (
				<ConnectionDetailDialog
					provider={{
						...connectionDetail,
						models: catalog.connectionModels[connectionDetail.id] ?? {},
					}}
					connectionModels={catalog.connectionModels[connectionDetail.id] ?? {}}
					open={!!connectionDetail}
					onOpenChange={(open) => { if (!open) setConnectionDetail(null) }}
					onChanged={handleSaved}
				/>
			)}

			{/* Remove connection confirmation */}
			<Dialog open={!!disconnectTarget} onOpenChange={(open) => { if (!open) setDisconnectTarget(null) }}>
				<DialogContent className="sm:max-w-lg gap-5 p-5">
					<DialogHeader className="gap-1.5">
						<DialogTitle className="text-base font-medium tracking-tight">
							Remove {disconnectTarget?.name}?
						</DialogTitle>
						<DialogDescription className="text-sm leading-5">
							Removes this connection and its credential. The built-in template stays available to reconnect.
						</DialogDescription>
					</DialogHeader>
					<DialogFooter className="gap-2 sm:justify-end">
						<Button variant="outline" size="sm" onClick={() => setDisconnectTarget(null)}>
							Cancel
						</Button>
						<Button variant="destructive" size="sm" disabled={disconnecting} onClick={handleDisconnect}>
							{disconnecting ? "Removing…" : "Remove"}
						</Button>
					</DialogFooter>
				</DialogContent>
			</Dialog>
		</div>
	)
}

// ============================================================
// Page header
// ============================================================

function ProviderSettingsPageHeader({ onAddCustom }: { onAddCustom: () => void }) {
	return (
		<SettingsHeader
			title="Providers"
			description="Connect AI providers and manage their credentials."
			action={
				<Button variant="secondary" size="sm" onClick={onAddCustom}>
					<PlusIcon data-icon="inline-start" />
					Custom
				</Button>
			}
		/>
	)
}

// ============================================================
// Row components
// ============================================================

function providerSubtitle(
	provider: CatalogProviderInfo,
	modelCount: number,
	options?: { includeBaseUrl?: boolean },
): string {
	const description =
		typeof provider.description === "string" ? provider.description.trim() : ""
	if (description) {
		if (options?.includeBaseUrl && provider.baseUrl) {
			return `${description} · ${provider.baseUrl}`
		}
		return description
	}
	const modelsLabel = `${modelCount} model${modelCount !== 1 ? "s" : ""}`
	if (options?.includeBaseUrl && provider.baseUrl) {
		return `${modelsLabel} · ${provider.baseUrl}`
	}
	return modelsLabel
}

function ConnectedProviderRow({
	provider,
	modelCount,
	onOpen,
	onDisconnect,
}: {
	provider: CatalogProviderInfo
	modelCount: number
	onOpen: () => void
	onDisconnect: () => void
}) {
	return (
		<div className="flex items-center gap-3 px-4 py-3">
			<ProviderIcon id={provider.id} name={provider.name} size="xs" />
			<button type="button" className="min-w-0 flex-1 text-left" onClick={onOpen}>
				<span className="text-sm tracking-tight">{provider.name}</span>
				<p className="mt-0.5 truncate text-xs text-muted-foreground">
					{providerSubtitle(provider, modelCount, { includeBaseUrl: true })}
				</p>
			</button>
			<Button
				variant="ghost"
				size="sm"
				className="text-muted-foreground hover:text-destructive"
				onClick={onDisconnect}
			>
				<MinusIcon className="size-3.5 stroke-[1.5]" />
				Remove
			</Button>
		</div>
	)
}

function TemplateProviderRow({
	provider,
	onConnect,
}: {
	provider: CatalogProviderInfo
	onConnect: () => void
}) {
	const modelCount = Object.keys(provider.models ?? {}).length
	return (
		<div className="flex items-center gap-3 px-4 py-3">
			<ProviderIcon id={provider.id} name={provider.name} size="xs" />
			<div className="min-w-0 flex-1">
				<span className="text-sm tracking-tight">{provider.name}</span>
				<p className="mt-0.5 truncate text-xs text-muted-foreground">
					{providerSubtitle(provider, modelCount)}
				</p>
			</div>
			<Button variant="outline" size="sm" onClick={onConnect}>
				<PlusIcon className="size-3.5 stroke-[1.5]" />
				Connect
			</Button>
		</div>
	)
}

// ============================================================
// Loading skeleton
// ============================================================

function ProviderSettingsLoading() {
	return (
		<div className={settingsPageClass}>
			<ProviderSettingsPageHeader onAddCustom={() => {}} />
			<SettingsSection title="Connected">
				{[0, 1].map((index) => (
					<div key={index} className="flex items-center gap-3 px-4 py-3">
						<Skeleton className="size-5 rounded-md" />
						<div className="flex flex-1 flex-col gap-2">
							<Skeleton className="h-4 w-32" />
							<Skeleton className="h-3 w-56" />
						</div>
						<Skeleton className="h-8 w-20" />
					</div>
				))}
			</SettingsSection>
			<SettingsSection title="Popular Providers">
				{[0, 1, 2, 3].map((index) => (
					<div key={index} className="flex items-center gap-3 px-4 py-3">
						<Skeleton className="size-5 rounded-md" />
						<div className="flex flex-1 flex-col gap-2">
							<Skeleton className="h-4 w-28" />
							<Skeleton className="h-3 w-40" />
						</div>
						<Skeleton className="h-8 w-20" />
					</div>
				))}
			</SettingsSection>
		</div>
	)
}
