/**
 * Dialog for connecting a built-in provider template.
 *
 * The template supplies defaults for endpoint and protocol. The user can
 * override the Base URL and supply an API key before confirming.
 */

import type { CatalogProviderInfo } from "@devo-ai/sdk/v2/client"
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
import { Spinner } from "@devo/ui/components/spinner"
import { CheckCircle2Icon } from "lucide-react"
import { useCallback, useEffect, useState } from "react"
import { PROVIDER_KEY_URLS, PROVIDERS_OPTIONAL_API_KEY } from "../../lib/providers"
import { getBaseClient } from "../../services/connection-manager"
import { ProviderIcon } from "./provider-icon"

interface TemplateConnectDialogProps {
	provider: CatalogProviderInfo
	open: boolean
	onOpenChange: (open: boolean) => void
	onConnected: () => void
}

type ConnectState =
	| { status: "idle" }
	| { status: "saving" }
	| { status: "success" }
	| { status: "error"; message: string }

export function TemplateConnectDialog({
	provider,
	open,
	onOpenChange,
	onConnected,
}: TemplateConnectDialogProps) {
	const [apiKey, setApiKey] = useState("")
	const [baseUrl, setBaseUrl] = useState(provider.baseUrl ?? "")
	const [state, setState] = useState<ConnectState>({ status: "idle" })

	useEffect(() => {
		if (!open) return
		setApiKey("")
		setBaseUrl(provider.baseUrl ?? "")
		setState({ status: "idle" })
	}, [open, provider.baseUrl, provider.id])

	const keyUrl = PROVIDER_KEY_URLS[provider.id]
	const apiKeyOptional = PROVIDERS_OPTIONAL_API_KEY.has(provider.id)
	const busy = state.status === "saving"
	const trimmedBaseUrl = baseUrl.trim()

	const handleConnect = useCallback(async () => {
		if (!trimmedBaseUrl) {
			setState({ status: "error", message: "Base URL is required" })
			return
		}
		setState({ status: "saving" })
		try {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			// Local runtimes like Ollama must not seed the Connection with
			// bundled placeholder model ids — those 404 on chat. Connect empty
			// and refresh from the live directory instead.
			const seedLiveDirectory = PROVIDERS_OPTIONAL_API_KEY.has(provider.id)
			const connectionProvider: CatalogProviderInfo = {
				...provider,
				baseUrl: trimmedBaseUrl,
				...(seedLiveDirectory ? { models: {} } : {}),
			}
			await client.provider.upsert({
				provider: connectionProvider,
				...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
			})
			if (seedLiveDirectory) {
				try {
					await client.provider.discover({
						providerId: provider.id,
						forceRefresh: true,
					})
				} catch (discoverError) {
					// Connection still succeeds; user can retry Discover later.
					console.warn("provider discover after connect failed", discoverError)
				}
			}
			setState({ status: "success" })
			setTimeout(() => {
				onOpenChange(false)
				onConnected()
			}, 500)
		} catch (err) {
			setState({
				status: "error",
				message: err instanceof Error ? err.message : "Failed to connect",
			})
		}
	}, [provider, apiKey, trimmedBaseUrl, onOpenChange, onConnected])

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="sm:max-w-lg gap-0 p-0 overflow-hidden">
				<div className="border-b border-border/50 px-5 py-4">
					<DialogHeader className="gap-2">
						<div className="flex items-center gap-2.5">
							<ProviderIcon id={provider.id} name={provider.name} size="sm" />
							<DialogTitle className="text-base font-medium tracking-tight">
								Connect {provider.name}
							</DialogTitle>
						</div>
						<DialogDescription className="text-sm leading-5">
							Creates a Connection from this template. You can override the Base URL;
							model settings are configured under Models.
							{PROVIDERS_OPTIONAL_API_KEY.has(provider.id)
								? " Local providers refresh the model list from the live directory on connect."
								: null}
						</DialogDescription>
					</DialogHeader>
				</div>

				<div className="flex flex-col gap-4 px-5 py-4">
					<div className="flex flex-col gap-1.5">
						<Label htmlFor="connect-base-url" className="text-xs text-muted-foreground">
							Base URL
						</Label>
						<Input
							id="connect-base-url"
							value={baseUrl}
							onChange={(e) => setBaseUrl(e.target.value)}
							placeholder={provider.baseUrl ?? "https://…"}
							disabled={busy}
							className="h-9"
						/>
					</div>

					<div className="flex flex-col gap-1.5">
						<Label htmlFor="api-key" className="text-xs text-muted-foreground">
							API key
						</Label>
						<Input
							id="api-key"
							type="password"
							placeholder={apiKeyOptional ? "optional" : "sk-…"}
							value={apiKey}
							onChange={(e) => setApiKey(e.target.value)}
							disabled={busy}
							className="h-9"
						/>
						{keyUrl && (
							<a
								href={keyUrl.url}
								target="_blank"
								rel="noopener noreferrer"
								className="text-xs text-muted-foreground underline-offset-4 hover:underline"
							>
								{keyUrl.label} →
							</a>
						)}
					</div>

					{state.status === "error" && (
						<p className="text-sm text-destructive">{state.message}</p>
					)}
					{state.status === "success" && (
						<div className="flex items-center gap-2 text-sm text-emerald-600 dark:text-emerald-400">
							<CheckCircle2Icon className="size-3.5" />
							Connected — opening Models…
						</div>
					)}
				</div>

				<div className="flex items-center justify-end gap-2 border-t border-border/50 px-5 py-3">
					<Button variant="outline" size="sm" onClick={() => onOpenChange(false)} disabled={busy}>
						Cancel
					</Button>
					<Button size="sm" onClick={handleConnect} disabled={busy || !trimmedBaseUrl}>
						{busy && <Spinner className="size-3.5" />}
						{busy ? "Connecting…" : "Connect"}
					</Button>
				</div>
			</DialogContent>
		</Dialog>
	)
}
