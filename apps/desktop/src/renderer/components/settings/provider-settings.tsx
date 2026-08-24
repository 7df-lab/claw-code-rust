import type { ProviderVendor } from "@devo-ai/sdk/v2/client"
import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import {
	Empty,
	EmptyContent,
	EmptyDescription,
	EmptyHeader,
	EmptyMedia,
	EmptyTitle,
} from "@devo/ui/components/empty"
import { Skeleton } from "@devo/ui/components/skeleton"
import { AlertCircleIcon, PencilIcon, PlugZapIcon, PlusIcon, RefreshCwIcon } from "lucide-react"
import { useCallback, useState } from "react"
import { useProviderVendors } from "../../hooks/use-devo-data"
import { ProviderIcon } from "./provider-icon"
import { ProviderVendorDialog } from "./provider-vendor-dialog"
import { SettingsHeader } from "./settings-header"
import { SettingsSection } from "./settings-section"

interface ProviderSettingsViewProps {
	providerVendors: ProviderVendor[]
	loading: boolean
	error: string | null
	onReload: () => void
}

export function ProviderSettings() {
	const { data, loading, error, reload } = useProviderVendors()
	return (
		<ProviderSettingsView
			providerVendors={data ?? []}
			loading={loading}
			error={error}
			onReload={reload}
		/>
	)
}

export function ProviderSettingsView({
	providerVendors,
	loading,
	error,
	onReload,
}: ProviderSettingsViewProps) {
	const [dialogOpen, setDialogOpen] = useState(false)
	const [editingProvider, setEditingProvider] = useState<ProviderVendor | null>(null)

	const openAddDialog = useCallback(() => {
		setEditingProvider(null)
		setDialogOpen(true)
	}, [])

	const openEditDialog = useCallback((providerVendor: ProviderVendor) => {
		setEditingProvider(providerVendor)
		setDialogOpen(true)
	}, [])

	const handleDialogOpenChange = useCallback((open: boolean) => {
		setDialogOpen(open)
		if (!open) {
			setEditingProvider(null)
		}
	}, [])

	if (loading) {
		return <ProviderSettingsLoading />
	}

	if (error) {
		return (
			<div className="flex flex-col gap-10">
				<ProviderSettingsHeader onAddProvider={openAddDialog} />
				<div className="flex items-center gap-3 rounded-[18px] border border-destructive/40 bg-destructive/10 px-5 py-3.5 text-[15px] text-destructive">
					<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
					<span>Failed to load providers: {error}</span>
					<Button variant="outline" size="sm" className="ml-auto" onClick={onReload}>
						<RefreshCwIcon data-icon="inline-start" />
						Retry
					</Button>
				</div>
			</div>
		)
	}

	return (
		<div className="flex flex-col gap-10">
			<ProviderSettingsHeader onAddProvider={openAddDialog} />

			{providerVendors.length === 0 ? (
				<Empty className="rounded-[18px] border border-border/60 bg-background shadow-[0_8px_32px_rgba(0,0,0,0.05)]">
					<EmptyHeader>
						<EmptyMedia variant="icon">
							<PlugZapIcon aria-hidden="true" />
						</EmptyMedia>
						<EmptyTitle className="text-[22px] font-normal tracking-[-0.03em]">
							No providers configured
						</EmptyTitle>
						<EmptyDescription className="text-[15px] leading-6">
							Add a provider endpoint and model binding to make it available in Desktop.
						</EmptyDescription>
					</EmptyHeader>
					<EmptyContent>
						<Button className="h-9 rounded-full px-4" onClick={openAddDialog}>
							<PlusIcon data-icon="inline-start" />
							Add Provider
						</Button>
					</EmptyContent>
				</Empty>
			) : (
				<SettingsSection title="Configured Providers">
					{providerVendors.map((providerVendor) => (
						<ProviderVendorRow
							key={providerVendor.name}
							providerVendor={providerVendor}
							onEdit={() => openEditDialog(providerVendor)}
						/>
					))}
				</SettingsSection>
			)}

			{dialogOpen && (
				<ProviderVendorDialog
					providerVendor={editingProvider}
					open={dialogOpen}
					onOpenChange={handleDialogOpenChange}
					onSaved={onReload}
				/>
			)}
		</div>
	)
}

function ProviderSettingsHeader({ onAddProvider }: { onAddProvider: () => void }) {
	return (
		<SettingsHeader
			title="Providers"
			action={
				<Button variant="secondary" className="h-9 rounded-full px-4" onClick={onAddProvider}>
					<PlusIcon data-icon="inline-start" />
					Add Provider
				</Button>
			}
			description={
				<>
					Connect AI providers to use their models.{" "}
					<a
						href="https://devo.ai/docs/providers/"
						target="_blank"
						rel="noopener noreferrer"
						className="text-foreground/80 underline-offset-4 hover:underline"
					>
						Learn more &rsaquo;
					</a>
				</>
			}
		/>
	)
}

function ProviderVendorRow({
	providerVendor,
	onEdit,
}: {
	providerVendor: ProviderVendor
	onEdit: () => void
}) {
	const wireApis = providerVendor.wire_apis.join(", ")
	const endpoint = providerVendor.base_url ?? "Provider default endpoint"

	return (
		<div className="flex items-center gap-3 px-5 py-3.5">
			<ProviderIcon id={providerVendor.name} name={providerVendor.name} />
			<div className="min-w-0 flex-1">
				<div className="flex flex-wrap items-center gap-2">
					<span className="text-[15px] font-normal tracking-[-0.01em]">{providerVendor.name}</span>
					<Badge variant={providerVendor.enabled ? "secondary" : "outline"}>
						{providerVendor.enabled ? "Enabled" : "Disabled"}
					</Badge>
				</div>
				<div className="mt-1 flex flex-wrap items-center gap-x-3 gap-y-1 text-[13px] text-muted-foreground">
					<span>{endpoint}</span>
					<span>{wireApis}</span>
				</div>
			</div>
			<Button variant="outline" size="sm" className="rounded-full" onClick={onEdit}>
				<PencilIcon data-icon="inline-start" />
				Edit
			</Button>
		</div>
	)
}

function ProviderSettingsLoading() {
	return (
		<div className="flex flex-col gap-10">
			<ProviderSettingsHeader onAddProvider={() => {}} />
			<SettingsSection title="Configured Providers">
				{[0, 1, 2].map((index) => (
					<div key={index} className="flex items-center gap-3 px-5 py-3.5">
						<Skeleton className="size-8 rounded-full" />
						<div className="flex flex-1 flex-col gap-2">
							<Skeleton className="h-4 w-32" />
							<Skeleton className="h-3 w-56" />
						</div>
						<Skeleton className="h-8 w-16" />
					</div>
				))}
			</SettingsSection>
		</div>
	)
}
