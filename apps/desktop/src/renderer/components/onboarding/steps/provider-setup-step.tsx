/**
 * Onboarding: Provider Setup.
 *
 * Allows users to connect AI providers during onboarding.
 * Devo Zen is featured prominently as the built-in provider with free models.
 * Reuses ConnectProviderDialog for the actual auth flows.
 */

import { Button } from "@devo/ui/components/button"
import { Spinner } from "@devo/ui/components/spinner"
import { useQueryClient } from "@tanstack/react-query"
import { CheckIcon, ExternalLinkIcon, LinkIcon, SparklesIcon, ZapIcon } from "lucide-react"
import { motion } from "motion/react"
import { useCallback, useMemo, useState } from "react"
import {
	type CatalogProvider,
	queryKeys,
	useAllProviders,
	useConnectedProviders,
	useProviderAuthMethods,
} from "../../../hooks/use-devo-data"
import { useServerConnection } from "../../../hooks/use-server"
import {
	compareConnectedFirst,
	isZenFreeTier,
	POPULAR_PROVIDER_IDS,
	ZEN_PROVIDER_ID,
	ZEN_SIGNUP_URL,
} from "../../../lib/providers"
import { ConnectProviderDialog } from "../../settings/connect-provider-dialog"
import { ProviderIcon } from "../../settings/provider-icon"

// ============================================================
// Component
// ============================================================

interface ProviderSetupStepProps {
	onComplete: (count: number) => void
	onSkip: () => void
}

export function ProviderSetupStep({ onComplete, onSkip }: ProviderSetupStepProps) {
	const { connected: serverConnected } = useServerConnection()
	const { data: allProviders, loading: catalogLoading, reload: reloadCatalog } = useAllProviders()
	const { loading: connectedLoading, reload: reloadConnected } = useConnectedProviders()
	const { data: authMethods } = useProviderAuthMethods()
	const queryClient = useQueryClient()

	const [connectDialogProvider, setConnectDialogProvider] = useState<CatalogProvider | null>(null)

	const loading = catalogLoading || connectedLoading
	const connectedIds = useMemo(
		() => new Set(allProviders?.connected ?? []),
		[allProviders?.connected],
	)

	// Separate Zen from the other popular providers
	const zenProvider = useMemo(
		() => allProviders?.all.find((p) => p.id === ZEN_PROVIDER_ID) ?? null,
		[allProviders],
	)

	const otherProviders = useMemo(() => {
		if (!allProviders) return []
		const filtered = allProviders.all.filter(
			(p) =>
				p.id !== ZEN_PROVIDER_ID &&
				POPULAR_PROVIDER_IDS.includes(p.id as (typeof POPULAR_PROVIDER_IDS)[number]),
		)
		return [...filtered].sort((a, b) => compareConnectedFirst(connectedIds, a, b))
	}, [allProviders, connectedIds])

	const zenIsConnected = connectedIds.has(ZEN_PROVIDER_ID)
	const zenHasApiKey = zenIsConnected && zenProvider !== null && !isZenFreeTier(zenProvider.models)

	const reload = useCallback(() => {
		reloadCatalog()
		reloadConnected()
		queryClient.invalidateQueries({ queryKey: queryKeys.allProviders })
		queryClient.invalidateQueries({ queryKey: queryKeys.connectedProviders })
		queryClient.invalidateQueries({
			predicate: (q) => q.queryKey[0] === "providers",
		})
	}, [reloadCatalog, reloadConnected, queryClient])

	const handleContinue = useCallback(() => {
		onComplete(connectedIds.size)
	}, [onComplete, connectedIds.size])

	if (!serverConnected) {
		return (
			<div className="flex h-full flex-col items-center justify-center space-y-6 text-center">
				<div className="flex flex-col items-center space-y-2">
					<Spinner className="size-8 text-muted-foreground" />
					<h2 className="text-[22px] font-medium tracking-tight">Waiting for Devo server...</h2>
					<p className="max-w-md text-sm text-muted-foreground">
						Devo is connecting to the Devo background process. This should only take a moment.
					</p>
				</div>
				<div className="flex gap-3">
					<Button variant="outline" onClick={onSkip}>
						Skip for now
					</Button>
				</div>
			</div>
		)
	}

	return (
		<div className="flex h-full flex-col items-center justify-center space-y-8 px-6 text-center">
			<div className="max-w-md space-y-2">
				<motion.div
					initial={{ scale: 0.9, opacity: 0 }}
					animate={{ scale: 1, opacity: 1 }}
					transition={{ duration: 0.4, ease: "easeOut" }}
					className="mx-auto flex size-12 items-center justify-center rounded-xl bg-primary/10 text-primary"
				>
					<SparklesIcon className="size-6" />
				</motion.div>
				<h2 className="text-2xl font-bold tracking-tight">AI Providers</h2>
				<p className="text-muted-foreground">
					Devo supports connecting your own model API keys for more model choices.
				</p>
			</div>

			<div className="w-full max-w-lg space-y-4">
				{/* Zen featured card */}
				{loading ? (
					<div className="flex items-center gap-4 rounded-xl border border-border bg-muted/20 p-4">
						<div className="size-10 animate-pulse rounded-lg bg-muted" />
						<div className="flex-1 space-y-2">
							<div className="h-4 w-32 animate-pulse rounded bg-muted" />
							<div className="h-3 w-48 animate-pulse rounded bg-muted" />
						</div>
					</div>
				) : zenProvider ? (
					<ZenFeaturedCard
						provider={zenProvider}
						hasApiKey={zenHasApiKey}
						onConnect={() => setConnectDialogProvider(zenProvider)}
					/>
				) : null}

				{/* Other providers grid */}
				<div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
					{loading
						? ["s1", "s2", "s3", "s4", "s5", "s6"].map((key) => (
								<div
									key={key}
									className="flex items-center gap-3 rounded-xl border border-border bg-muted/20 px-4 py-3"
								>
									<div className="size-8 animate-pulse rounded-md bg-muted" />
									<div className="h-4 w-24 animate-pulse rounded bg-muted" />
								</div>
							))
						: otherProviders.map((provider) => {
								const isConnected = connectedIds.has(provider.id)
								return (
									<button
										key={provider.id}
										type="button"
										onClick={() => setConnectDialogProvider(provider)}
										className="group flex items-center gap-3 rounded-xl border border-border bg-background px-4 py-3 text-left transition-all hover:border-primary/50 hover:bg-accent"
									>
										<ProviderIcon id={provider.id} name={provider.name} />
										<div className="flex flex-1 flex-col">
											<span className="text-sm font-medium">{provider.name}</span>
											{isConnected && (
												<span className="text-[10px] font-semibold uppercase tracking-wider text-emerald-600 dark:text-emerald-400">
													Connected
												</span>
											)}
										</div>
										{isConnected ? (
											<CheckIcon className="size-4 text-emerald-500" />
										) : (
											<LinkIcon className="size-4 text-muted-foreground opacity-0 transition-opacity group-hover:opacity-100" />
										)}
									</button>
								)
							})}
				</div>

				<div className="flex flex-col items-center gap-4 pt-4">
					<Button size="lg" className="min-w-40" onClick={handleContinue} disabled={loading}>
						{connectedIds.size > 0 || zenIsConnected ? "Continue" : "I'll do this later"}
					</Button>
					{connectedIds.size === 0 && !zenIsConnected && (
						<p className="text-xs text-muted-foreground">
							You won't be able to chat until a provider is connected.
						</p>
					)}
				</div>
			</div>

			<ConnectProviderDialog
				provider={connectDialogProvider}
				pluginAuthMethods={
					connectDialogProvider ? authMethods?.[connectDialogProvider.id] : undefined
				}
				onClose={() => setConnectDialogProvider(null)}
				onConnected={() => {
					setConnectDialogProvider(null)
					reload()
				}}
			/>
		</div>
	)
}

// ============================================================
// Zen featured card
// ============================================================

function ZenFeaturedCard({
	provider,
	hasApiKey,
	onConnect,
}: {
	provider: CatalogProvider
	hasApiKey: boolean
	onConnect: () => void
}) {
	const freeModelCount = Object.values(provider.models).filter(
		(m) => (m as { cost?: { input?: number } }).cost?.input === 0,
	).length
	const totalModelCount = Object.keys(provider.models).length

	return (
		<motion.div
			initial={{ opacity: 0, y: 8 }}
			animate={{ opacity: 1, y: 0 }}
			transition={{ duration: 0.3, ease: "easeOut" }}
			className="relative overflow-hidden rounded-xl border border-primary/20 bg-gradient-to-br from-primary/[0.04] to-primary/[0.08]"
		>
			<div className="flex items-start gap-4 p-4">
				<div className="flex size-10 shrink-0 items-center justify-center rounded-lg bg-primary/10">
					<ProviderIcon id={provider.id} name={provider.name} size="md" />
				</div>
				<div className="min-w-0 flex-1 text-left">
					<div className="flex items-center gap-2">
						<span className="text-sm font-semibold">{provider.name}</span>
						<span className="inline-flex items-center gap-1 rounded-full bg-emerald-500/10 px-2 py-0.5 text-[10px] font-semibold uppercase tracking-wider text-emerald-600 dark:text-emerald-400">
							<ZapIcon className="size-2.5" aria-hidden="true" />
							Included
						</span>
					</div>
					<p className="mt-0.5 text-xs text-muted-foreground">
						{freeModelCount} free models ready to use.{" "}
						{hasApiKey
							? `${totalModelCount} models available with API key.`
							: `Upgrade for ${totalModelCount}+ premium models.`}
					</p>
					<div className="mt-2.5 flex flex-wrap items-center gap-2">
						<Button size="sm" variant="outline" className="h-7 text-xs" onClick={onConnect}>
							{hasApiKey ? "Manage" : "Enter API key"}
						</Button>
						{!hasApiKey && (
							<a
								href={ZEN_SIGNUP_URL}
								target="_blank"
								rel="noopener noreferrer"
								className="inline-flex items-center gap-1 text-[11px] text-primary hover:underline"
							>
								Get a key at devo.ai
								<ExternalLinkIcon className="size-2.5" aria-hidden="true" />
							</a>
						)}
					</div>
				</div>
			</div>
		</motion.div>
	)
}
