/**
 * Dialog for connecting an AI provider via API key or OAuth.
 *
 * Supports multi-step flows: method selection -> authentication.
 * For multi-env providers, shows either a structured credential form
 * (when auth.set + config options can cover all fields) or improved
 * env-var instructions with copy buttons and docs links.
 */

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
import { Label } from "@devo/ui/components/label"
import { Spinner } from "@devo/ui/components/spinner"
import {
	AlertCircleIcon,
	CheckCircle2Icon,
	CheckIcon,
	ClipboardIcon,
	ExternalLinkIcon,
	KeyIcon,
	SparklesIcon,
	TerminalIcon,
	ZapIcon,
} from "lucide-react"
import { useCallback, useEffect, useRef, useState } from "react"
import type {
	CatalogProvider,
	SdkProviderAuthMethod as ProviderAuthMethod,
} from "../../hooks/use-devo-data"
import { createLogger } from "../../lib/logger"
import { PROVIDER_KEY_URLS, ZEN_PROVIDER_ID, ZEN_SIGNUP_URL } from "../../lib/providers"
import { getBaseClient } from "../../services/connection-manager"
import { ProviderIcon } from "./provider-icon"

const log = createLogger("connect-provider-dialog")

// ============================================================
// Types
// ============================================================

type DialogStep =
	| { type: "select-method" }
	| { type: "api-key" }
	| { type: "configure" }
	| { type: "env-setup" }
	| { type: "zen-setup" }
	| {
			type: "oauth"
			method: number
			url?: string
			oauthMethod?: "auto" | "code"
			instructions?: string
	  }

type DialogState =
	| { status: "idle" }
	| { status: "loading" }
	| { status: "success" }
	| { status: "error"; message: string }

// ============================================================
// Provider configuration definitions
// ============================================================

/** A field in the provider configuration form */
interface ProviderField {
	/** Unique key -- used as the config option key or "apiKey" for auth.set */
	key: string
	/** Display label */
	label: string
	/** Placeholder text */
	placeholder: string
	/** Whether this is a secret (password input) */
	secret?: boolean
	/** Whether this field is required */
	required?: boolean
	/** How to persist this value */
	persist: "auth" | "config"
	/** Help text shown below the field */
	help?: string
}

/** Configuration for a provider that can be set up via a form */
interface ConfigurableProvider {
	/** Provider ID */
	id: string
	/** Form fields in display order */
	fields: ProviderField[]
	/** Devo docs anchor (appended to https://devo.ai/docs/providers/#) */
	docsAnchor?: string
}

/**
 * Providers that can be fully configured via auth.set() + config.update().
 * These show a multi-field form instead of env-var instructions.
 */
const CONFIGURABLE_PROVIDERS: ConfigurableProvider[] = [
	{
		id: "azure",
		docsAnchor: "azure-openai",
		fields: [
			{
				key: "apiKey",
				label: "API Key",
				placeholder: "your-azure-api-key",
				secret: true,
				required: true,
				persist: "auth",
			},
			{
				key: "baseURL",
				label: "Endpoint URL",
				placeholder: "https://your-resource.openai.azure.com/openai",
				required: true,
				persist: "config",
				help: "Your Azure OpenAI resource endpoint",
			},
		],
	},
	{
		id: "azure-cognitive-services",
		docsAnchor: "azure-cognitive-services",
		fields: [
			{
				key: "apiKey",
				label: "API Key",
				placeholder: "your-azure-api-key",
				secret: true,
				required: true,
				persist: "auth",
			},
			{
				key: "baseURL",
				label: "Endpoint URL",
				placeholder: "https://your-resource.cognitiveservices.azure.com/openai",
				required: true,
				persist: "config",
				help: "Your Azure Cognitive Services resource endpoint",
			},
		],
	},
	{
		id: "amazon-bedrock",
		docsAnchor: "amazon-bedrock",
		fields: [
			{
				key: "apiKey",
				label: "Bearer Token",
				placeholder: "your-bearer-token",
				secret: true,
				required: true,
				persist: "auth",
				help: "AWS bearer token for Bedrock access",
			},
			{
				key: "region",
				label: "AWS Region",
				placeholder: "us-east-1",
				required: true,
				persist: "config",
			},
			{
				key: "profile",
				label: "AWS Profile",
				placeholder: "default",
				persist: "config",
				help: "Optional AWS profile name",
			},
		],
	},
]

const CONFIGURABLE_PROVIDER_MAP = new Map(CONFIGURABLE_PROVIDERS.map((p) => [p.id, p]))

/**
 * Devo docs anchors for providers that are NOT configurable via form.
 * Used to link to provider-specific setup instructions.
 */
const PROVIDER_DOCS_ANCHORS: Record<string, string> = {
	"cloudflare-workers-ai": "cloudflare-workers-ai",
	"cloudflare-ai-gateway": "cloudflare-ai-gateway",
	"google-vertex": "google-vertex-ai",
	"google-vertex-anthropic": "google-vertex-ai",
	"sap-ai-core": "sap-ai-core",
	"amazon-bedrock": "amazon-bedrock",
	azure: "azure-openai",
	"azure-cognitive-services": "azure-cognitive-services",
	gitlab: "gitlab-duo",
	"privatemode-ai": "custom-provider",
}

/** Build a provider docs URL from an anchor */
function getProviderDocsUrl(providerId: string): string | null {
	const configurable = CONFIGURABLE_PROVIDER_MAP.get(providerId)
	const anchor = configurable?.docsAnchor ?? PROVIDER_DOCS_ANCHORS[providerId]
	if (!anchor) return null
	return `https://devo.ai/docs/providers/#${anchor}`
}

// ============================================================
// Main component
// ============================================================

/** Default auth method when no plugin provides methods for a provider */
const DEFAULT_API_KEY_METHOD: ProviderAuthMethod[] = [{ type: "api", label: "API Key" }]

/**
 * Providers whose env arrays list alternative names for the same credential.
 * These should show a single API key input, not an env-setup view.
 * All other providers with env.length > 1 are treated as co-required.
 */
const ALTERNATIVE_ENV_PROVIDERS = new Set(["google"])

/**
 * Determines the setup type for a multi-env provider:
 * - "configure": can be fully set up via auth.set() + config options
 * - "env-setup": must use environment variables (at least partially)
 * - null: not a multi-env provider
 */
function getMultiEnvSetupType(provider: CatalogProvider): "configure" | "env-setup" | null {
	if (provider.env.length <= 1 || ALTERNATIVE_ENV_PROVIDERS.has(provider.id)) return null
	if (CONFIGURABLE_PROVIDER_MAP.has(provider.id)) return "configure"
	return "env-setup"
}

interface ConnectProviderDialogProps {
	provider: CatalogProvider | null
	/** Plugin-provided auth methods. Falls back to API key if empty/undefined. */
	pluginAuthMethods?: ProviderAuthMethod[]
	onClose: () => void
	onConnected: () => void
}

export function ConnectProviderDialog({
	provider,
	pluginAuthMethods,
	onClose,
	onConnected,
}: ConnectProviderDialogProps) {
	const open = provider !== null
	const [step, setStep] = useState<DialogStep>({ type: "select-method" })
	const [state, setState] = useState<DialogState>({ status: "idle" })

	// Use plugin methods if available, otherwise default to API key
	const authMethods =
		pluginAuthMethods && pluginAuthMethods.length > 0 ? pluginAuthMethods : DEFAULT_API_KEY_METHOD

	// Reset state when dialog opens/closes
	useEffect(() => {
		if (open) {
			setState({ status: "idle" })

			// Devo Zen: show the special Zen setup view
			if (provider?.id === ZEN_PROVIDER_ID) {
				setStep({ type: "zen-setup" })
				return
			}

			// Multi-env providers: route to either a form or env-var instructions
			if (provider && authMethods === DEFAULT_API_KEY_METHOD) {
				const setupType = getMultiEnvSetupType(provider)
				if (setupType === "configure") {
					setStep({ type: "configure" })
					return
				}
				if (setupType === "env-setup") {
					setStep({ type: "env-setup" })
					return
				}
			}

			// Auto-select method if only one available
			if (authMethods.length === 1) {
				const method = authMethods[0]
				if (method.type === "api") {
					setStep({ type: "api-key" })
				} else {
					setStep({ type: "oauth", method: 0 })
				}
			} else {
				setStep({ type: "select-method" })
			}
		}
	}, [open, authMethods, provider])

	const handleOpenChange = useCallback(
		(isOpen: boolean) => {
			if (!isOpen) onClose()
		},
		[onClose],
	)

	const handleOAuthSuccess = useCallback(() => {
		setState({ status: "success" })
	}, [])

	if (!provider) return null

	return (
		<Dialog open={open} onOpenChange={handleOpenChange}>
			<DialogContent showCloseButton={false} className="sm:max-w-md">
				<DialogHeader>
					<DialogTitle className="flex items-center gap-3">
						<ProviderIcon id={provider.id} name={provider.name} />
						{provider.id === ZEN_PROVIDER_ID ? provider.name : `Connect ${provider.name}`}
					</DialogTitle>
					<DialogDescription>
						{provider.id === ZEN_PROVIDER_ID
							? "Curated, optimized models for coding agents with a single API key."
							: `Add your credentials to start using ${provider.name} models`}
					</DialogDescription>
				</DialogHeader>

				{state.status === "success" ? (
					<SuccessView provider={provider} onDone={onConnected} />
				) : step.type === "zen-setup" ? (
					<ZenSetupView
						provider={provider}
						state={state}
						onSubmit={async (apiKey) => {
							setState({ status: "loading" })
							try {
								const client = getBaseClient()
								if (!client) throw new Error("Not connected to server")
								await client.auth.set({
									providerID: provider.id,
									auth: { type: "api", key: apiKey },
								})
								await client.global.dispose()
								setState({ status: "success" })
							} catch (err) {
								const message = err instanceof Error ? err.message : "Failed to connect"
								log.error("Failed to set API key", {
									provider: provider.id,
									error: err,
								})
								setState({ status: "error", message })
							}
						}}
						onCancel={onClose}
					/>
				) : step.type === "configure" ? (
					<ConfigureProviderView
						provider={provider}
						state={state}
						setState={setState}
						onCancel={onClose}
					/>
				) : step.type === "env-setup" ? (
					<EnvSetupView provider={provider} onCancel={onClose} />
				) : step.type === "select-method" ? (
					<MethodSelectView
						authMethods={authMethods ?? []}
						onSelectApiKey={() => setStep({ type: "api-key" })}
						onSelectOAuth={(methodIndex) => setStep({ type: "oauth", method: methodIndex })}
					/>
				) : step.type === "api-key" ? (
					<ApiKeyView
						provider={provider}
						state={state}
						onSubmit={async (apiKey) => {
							setState({ status: "loading" })
							try {
								const client = getBaseClient()
								if (!client) throw new Error("Not connected to server")
								await client.auth.set({
									providerID: provider.id,
									auth: { type: "api", key: apiKey },
								})
								await client.global.dispose()
								setState({ status: "success" })
							} catch (err) {
								const message = err instanceof Error ? err.message : "Failed to connect"
								log.error("Failed to set API key", {
									provider: provider.id,
									error: err,
								})
								setState({ status: "error", message })
							}
						}}
						onBack={
							authMethods && authMethods.length > 1
								? () => {
										setStep({ type: "select-method" })
										setState({ status: "idle" })
									}
								: undefined
						}
						onCancel={onClose}
					/>
				) : step.type === "oauth" ? (
					<OAuthView
						provider={provider}
						methodIndex={step.method}
						state={state}
						setState={setState}
						onSuccess={handleOAuthSuccess}
						onBack={
							authMethods && authMethods.length > 1
								? () => {
										setStep({ type: "select-method" })
										setState({ status: "idle" })
									}
								: undefined
						}
						onCancel={onClose}
					/>
				) : null}
			</DialogContent>
		</Dialog>
	)
}

// ============================================================
// Views
// ============================================================

function MethodSelectView({
	authMethods,
	onSelectApiKey,
	onSelectOAuth,
}: {
	authMethods: ProviderAuthMethod[]
	onSelectApiKey: () => void
	onSelectOAuth: (methodIndex: number) => void
}) {
	return (
		<div className="space-y-2 py-2">
			<p className="text-sm text-muted-foreground mb-3">Choose an authentication method:</p>
			{authMethods.map((method, index) => (
				<button
					key={`${method.type}-${index}`}
					type="button"
					onClick={() => {
						if (method.type === "api") {
							onSelectApiKey()
						} else {
							onSelectOAuth(index)
						}
					}}
					className="flex w-full items-center gap-3 rounded-lg border border-border px-4 py-3 text-left transition-colors hover:bg-accent"
				>
					{method.type === "api" ? (
						<KeyIcon className="size-4 text-muted-foreground" aria-hidden="true" />
					) : (
						<ExternalLinkIcon className="size-4 text-muted-foreground" aria-hidden="true" />
					)}
					<div>
						<div className="text-sm font-medium">{method.label}</div>
						<div className="text-xs text-muted-foreground">
							{method.type === "api" ? "Enter your API key" : "Sign in via your browser"}
						</div>
					</div>
				</button>
			))}
		</div>
	)
}

function EnvSetupView({ provider, onCancel }: { provider: CatalogProvider; onCancel: () => void }) {
	const docsUrl = getProviderDocsUrl(provider.id)

	return (
		<div className="space-y-4 py-2">
			<div className="flex items-start gap-3 rounded-lg border border-border bg-muted/50 px-4 py-3">
				<TerminalIcon className="mt-0.5 size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
				<div className="space-y-2 text-sm">
					<p className="font-medium">Environment variable setup required</p>
					<p className="text-muted-foreground">
						{provider.name} requires credentials that must be set as environment variables before
						starting the app.
					</p>
				</div>
			</div>

			<div className="space-y-1.5">
				{provider.env.map((envVar) => (
					<CopyableEnvVar key={envVar} envVar={envVar} />
				))}
			</div>

			<p className="text-xs text-muted-foreground">
				Add these to your shell profile (e.g.{" "}
				<code className="rounded bg-muted px-1 py-0.5 text-[11px]">~/.zshrc</code>) or a{" "}
				<code className="rounded bg-muted px-1 py-0.5 text-[11px]">.env</code> file, then restart
				the app. The provider will be detected automatically.
			</p>

			{docsUrl && (
				<a
					href={docsUrl}
					target="_blank"
					rel="noopener noreferrer"
					className="flex items-center gap-1.5 text-xs text-primary hover:underline"
				>
					<ExternalLinkIcon className="size-3" aria-hidden="true" />
					View setup guide on devo.ai
				</a>
			)}

			<DialogFooter>
				<Button type="button" variant="outline" onClick={onCancel}>
					Close
				</Button>
			</DialogFooter>
		</div>
	)
}

function ConfigureProviderView({
	provider,
	state,
	setState,
	onCancel,
}: {
	provider: CatalogProvider
	state: DialogState
	setState: (state: DialogState) => void
	onCancel: () => void
}) {
	const config = CONFIGURABLE_PROVIDER_MAP.get(provider.id)
	const docsUrl = getProviderDocsUrl(provider.id)

	// Initialize form values for each field
	const [values, setValues] = useState<Record<string, string>>(() => {
		const initial: Record<string, string> = {}
		for (const field of config?.fields ?? []) {
			initial[field.key] = ""
		}
		return initial
	})

	// Reset values when provider changes
	useEffect(() => {
		const initial: Record<string, string> = {}
		for (const field of config?.fields ?? []) {
			initial[field.key] = ""
		}
		setValues(initial)
	}, [config])

	const isLoading = state.status === "loading"

	const requiredFieldsFilled =
		config?.fields.every((f) => !f.required || values[f.key]?.trim()) ?? false

	const handleSubmit = useCallback(
		async (e: React.FormEvent) => {
			e.preventDefault()
			if (!config) return

			setState({ status: "loading" })
			try {
				const client = getBaseClient()
				if (!client) throw new Error("Not connected to server")

				// Separate auth fields from config fields
				const authField = config.fields.find((f) => f.persist === "auth")
				const configFields = config.fields.filter(
					(f) => f.persist === "config" && values[f.key]?.trim(),
				)

				// Set the API key / credential via auth.json
				if (authField && values[authField.key]?.trim()) {
					await client.auth.set({
						providerID: provider.id,
						auth: { type: "api", key: values[authField.key].trim() },
					})
				}

				// Set config options via global config
				if (configFields.length > 0) {
					const options: Record<string, string> = {}
					for (const field of configFields) {
						options[field.key] = values[field.key].trim()
					}
					await client.global.config.update({
						config: {
							provider: {
								[provider.id]: { options },
							},
						},
					})
				}

				await client.global.dispose()
				setState({ status: "success" })
			} catch (err) {
				const message = err instanceof Error ? err.message : "Failed to connect"
				log.error("Failed to configure provider", {
					provider: provider.id,
					error: err,
				})
				setState({ status: "error", message })
			}
		},
		[config, values, provider.id, setState],
	)

	if (!config) return null

	return (
		<form onSubmit={handleSubmit}>
			<div className="space-y-4 py-2">
				{config.fields.map((field) => (
					<div key={field.key} className="space-y-2">
						<Label htmlFor={`field-${field.key}`}>
							{field.label}
							{!field.required && (
								<span className="ml-1 text-xs font-normal text-muted-foreground">(optional)</span>
							)}
						</Label>
						<Input
							id={`field-${field.key}`}
							type={field.secret ? "password" : "text"}
							placeholder={field.placeholder}
							value={values[field.key] ?? ""}
							onChange={(e) => setValues((prev) => ({ ...prev, [field.key]: e.target.value }))}
							disabled={isLoading}
						/>
						{field.help && <p className="text-xs text-muted-foreground">{field.help}</p>}
					</div>
				))}

				{docsUrl && (
					<a
						href={docsUrl}
						target="_blank"
						rel="noopener noreferrer"
						className="flex items-center gap-1.5 text-xs text-primary hover:underline"
					>
						<ExternalLinkIcon className="size-3" aria-hidden="true" />
						View setup guide on devo.ai
					</a>
				)}

				{state.status === "error" && (
					<div className="flex items-center gap-2 text-sm text-destructive">
						<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
						<span>{state.message}</span>
					</div>
				)}
			</div>

			<DialogFooter className="mt-4">
				<Button type="button" variant="outline" onClick={onCancel} disabled={isLoading}>
					Cancel
				</Button>
				<Button type="submit" disabled={!requiredFieldsFilled || isLoading}>
					{isLoading ? (
						<>
							<Spinner className="size-4" />
							Connecting...
						</>
					) : (
						"Connect"
					)}
				</Button>
			</DialogFooter>
		</form>
	)
}

/** Env var row with a copy-to-clipboard button */
function CopyableEnvVar({ envVar }: { envVar: string }) {
	const [copied, setCopied] = useState(false)
	const exportLine = `export ${envVar}=`

	const handleCopy = useCallback(() => {
		navigator.clipboard.writeText(exportLine).then(() => {
			setCopied(true)
			setTimeout(() => setCopied(false), 2000)
		})
	}, [exportLine])

	return (
		<div className="flex items-center justify-between gap-2 rounded-md bg-muted px-3 py-2">
			<code className="font-mono text-xs">export {envVar}=...</code>
			<button
				type="button"
				onClick={handleCopy}
				className="shrink-0 rounded p-1 text-muted-foreground transition-colors hover:bg-background hover:text-foreground"
				aria-label={`Copy ${envVar} export line`}
			>
				{copied ? (
					<CheckIcon className="size-3.5 text-emerald-500" aria-hidden="true" />
				) : (
					<ClipboardIcon className="size-3.5" aria-hidden="true" />
				)}
			</button>
		</div>
	)
}

function ApiKeyView({
	provider,
	state,
	onSubmit,
	onBack,
	onCancel,
}: {
	provider: CatalogProvider
	state: DialogState
	onSubmit: (apiKey: string) => void
	onBack?: () => void
	onCancel: () => void
}) {
	const [apiKey, setApiKey] = useState("")
	const isLoading = state.status === "loading"

	const handleSubmit = useCallback(
		(e: React.FormEvent) => {
			e.preventDefault()
			if (apiKey.trim()) {
				onSubmit(apiKey.trim())
			}
		},
		[apiKey, onSubmit],
	)

	return (
		<form onSubmit={handleSubmit}>
			<div className="space-y-4 py-2">
				<div className="space-y-2">
					<Label htmlFor="api-key">API Key</Label>
					<Input
						id="api-key"
						type="password"
						placeholder="sk-..."
						value={apiKey}
						onChange={(e) => setApiKey(e.target.value)}
						disabled={isLoading}
						autoFocus
					/>
					{PROVIDER_KEY_URLS[provider.id] && (
						<a
							href={PROVIDER_KEY_URLS[provider.id].url}
							target="_blank"
							rel="noopener noreferrer"
							className="inline-flex items-center gap-1.5 text-xs text-primary hover:underline"
						>
							<ExternalLinkIcon className="size-2.5" aria-hidden="true" />
							{PROVIDER_KEY_URLS[provider.id].label} at{" "}
							{new URL(PROVIDER_KEY_URLS[provider.id].url).hostname}
						</a>
					)}
					{provider.env.length > 0 && (
						<p className="text-xs text-muted-foreground">
							You can also set this via the{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[11px]">{provider.env[0]}</code>{" "}
							environment variable
						</p>
					)}
				</div>

				{state.status === "error" && (
					<div className="flex items-center gap-2 text-sm text-destructive">
						<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
						<span>{state.message}</span>
					</div>
				)}
			</div>

			<DialogFooter className="mt-4">
				{onBack && (
					<Button type="button" variant="ghost" onClick={onBack} disabled={isLoading}>
						Back
					</Button>
				)}
				<Button type="button" variant="outline" onClick={onCancel} disabled={isLoading}>
					Cancel
				</Button>
				<Button type="submit" disabled={!apiKey.trim() || isLoading}>
					{isLoading ? (
						<>
							<Spinner className="size-4" />
							Connecting...
						</>
					) : (
						"Connect"
					)}
				</Button>
			</DialogFooter>
		</form>
	)
}

function OAuthView({
	provider,
	methodIndex,
	state,
	setState,
	onSuccess,
	onBack,
	onCancel,
}: {
	provider: CatalogProvider
	methodIndex: number
	state: DialogState
	setState: (state: DialogState) => void
	onSuccess: () => void
	onBack?: () => void
	onCancel: () => void
}) {
	const [authUrl, setAuthUrl] = useState<string | null>(null)
	const [oauthMethod, setOauthMethod] = useState<"auto" | "code" | null>(null)
	const [authInstructions, setAuthInstructions] = useState<string | null>(null)
	const [code, setCode] = useState("")
	const [copiedDeviceCode, setCopiedDeviceCode] = useState(false)
	const autoCopiedDeviceCodeRef = useRef<string | null>(null)

	const deviceCode = extractDeviceCode(authInstructions)

	const handleCopyDeviceCode = useCallback(async () => {
		if (!deviceCode) return
		await navigator.clipboard.writeText(deviceCode)
		setCopiedDeviceCode(true)
		setTimeout(() => setCopiedDeviceCode(false), 2000)
	}, [deviceCode])

	useEffect(() => {
		if (!authUrl) return

		const valueToCopy = deviceCode ?? authUrl
		if (!valueToCopy) return
		if (autoCopiedDeviceCodeRef.current === valueToCopy) return
		autoCopiedDeviceCodeRef.current = valueToCopy

		void navigator.clipboard
			.writeText(valueToCopy)
			.then(() => {
				setCopiedDeviceCode(true)
				setTimeout(() => setCopiedDeviceCode(false), 2000)
			})
			.catch(() => {
				autoCopiedDeviceCodeRef.current = null
			})
	}, [authUrl, deviceCode])

	// Start OAuth flow on mount
	useEffect(() => {
		let cancelled = false

		async function startOAuth() {
			setState({ status: "loading" })
			try {
				const client = getBaseClient()
				if (!client) throw new Error("Not connected to server")
				const result = await client.provider.oauth.authorize({
					providerID: provider.id,
					method: methodIndex,
				})
				if (cancelled) return

					const data = result.data as unknown as
					| {
							url: string
							method: "auto" | "code"
							instructions: string
					  }
					| undefined

				if (!data?.url) {
					throw new Error("No authorization URL returned")
				}

				setAuthUrl(data.url)
				setOauthMethod(data.method)
				setAuthInstructions(data.instructions)

				// Open the URL in the browser (Electron intercepts via setWindowOpenHandler)
				window.open(data.url, "_blank")

				setState({ status: "idle" })

				// For auto method, start polling
				if (data.method === "auto") {
					pollForCompletion(client, provider.id, methodIndex, setState, onSuccess, () => cancelled)
				}
			} catch (err) {
				if (cancelled) return
				const message = err instanceof Error ? err.message : "Failed to start OAuth"
				log.error("Failed to start OAuth", { provider: provider.id, error: err })
				setState({ status: "error", message })
			}
		}

		startOAuth()
		return () => {
			cancelled = true
		}
	}, [provider.id, methodIndex, setState, onSuccess])

	const handleCodeSubmit = useCallback(
		async (e: React.FormEvent) => {
			e.preventDefault()
			if (!code.trim()) return
			setState({ status: "loading" })
			try {
				const client = getBaseClient()
				if (!client) throw new Error("Not connected to server")
				await client.provider.oauth.callback({
					providerID: provider.id,
					method: methodIndex,
					code: code.trim(),
				})
				await client.global.dispose()
				onSuccess()
			} catch (err) {
				const message = err instanceof Error ? err.message : "Failed to complete OAuth"
				log.error("Failed to complete OAuth callback", {
					provider: provider.id,
					error: err,
				})
				setState({ status: "error", message })
			}
		},
		[code, provider.id, methodIndex, setState, onSuccess],
	)

	if (oauthMethod === "code" && authUrl) {
		return (
			<form onSubmit={handleCodeSubmit}>
				<div className="space-y-4 py-2">
					<p className="text-sm text-muted-foreground">
						A browser window has been opened. Sign in and paste the authorization code below.
					</p>
					{authInstructions && (
						<p className="rounded-md border border-border bg-muted/40 px-3 py-2 text-xs leading-relaxed text-muted-foreground whitespace-pre-wrap">
							{authInstructions}
						</p>
					)}
					<div className="space-y-2">
						<Label htmlFor="oauth-code">Authorization Code</Label>
						<Input
							id="oauth-code"
							type="text"
							placeholder="Paste code here..."
							value={code}
							onChange={(e) => setCode(e.target.value)}
							disabled={state.status === "loading"}
							autoFocus
						/>
					</div>
					{state.status === "error" && (
						<div className="flex items-center gap-2 text-sm text-destructive">
							<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
							<span>{state.message}</span>
						</div>
					)}
				</div>
				<DialogFooter className="mt-4">
					{onBack && (
						<Button
							type="button"
							variant="ghost"
							onClick={onBack}
							disabled={state.status === "loading"}
						>
							Back
						</Button>
					)}
					<Button
						type="button"
						variant="outline"
						onClick={onCancel}
						disabled={state.status === "loading"}
					>
						Cancel
					</Button>
					<Button type="submit" disabled={!code.trim() || state.status === "loading"}>
						{state.status === "loading" ? (
							<>
								<Spinner className="size-4" />
								Verifying...
							</>
						) : (
							"Submit"
						)}
					</Button>
				</DialogFooter>
			</form>
		)
	}

	// Auto method or waiting for OAuth to start
	return (
		<div className="space-y-4 py-2">
			{state.status === "loading" && !authUrl ? (
				<div className="flex items-center justify-center gap-2 py-6">
					<Spinner className="size-4" />
					<span className="text-sm text-muted-foreground">Starting authentication...</span>
				</div>
			) : state.status === "error" ? (
				<>
					<div className="flex items-center gap-2 text-sm text-destructive">
						<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
						<span>{state.message}</span>
					</div>
					<DialogFooter>
						{onBack && (
							<Button type="button" variant="ghost" onClick={onBack}>
								Back
							</Button>
						)}
						<Button type="button" variant="outline" onClick={onCancel}>
							Cancel
						</Button>
					</DialogFooter>
				</>
			) : (
				<>
					<div className="flex flex-col items-center gap-3 py-6">
						<Spinner className="size-5" />
						{deviceCode && (
							<div className="flex flex-col items-center gap-2 rounded-lg border border-border bg-muted/30 px-4 py-3">
								<p className="text-xs text-muted-foreground">Enter this code in the browser</p>
								<div className="flex items-center gap-2">
									<code className="rounded-md bg-background px-2.5 py-1.5 font-mono text-sm font-semibold tracking-[0.2em]">
										{deviceCode}
									</code>
									<Button
										type="button"
										variant="ghost"
										size="sm"
										className="h-8 w-8 p-0"
										onClick={handleCopyDeviceCode}
									>
										{copiedDeviceCode ? (
											<CheckIcon className="size-3.5 text-emerald-500" aria-hidden="true" />
										) : (
											<ClipboardIcon className="size-3.5" aria-hidden="true" />
										)}
									</Button>
								</div>
							</div>
						)}
						{authInstructions && !deviceCode && (
							<p className="max-w-full rounded-md border border-border bg-muted/40 px-3 py-2 text-xs leading-relaxed text-muted-foreground whitespace-pre-wrap">
								{authInstructions}
							</p>
						)}
						<p className="text-sm text-muted-foreground text-center">
							Waiting for authentication to complete in your browser...
						</p>
						{authUrl && (
							<Button variant="link" size="sm" onClick={() => window.open(authUrl, "_blank")}>
								<ExternalLinkIcon className="size-3.5" aria-hidden="true" />
								Open link again
							</Button>
						)}
					</div>
					<DialogFooter>
						<Button type="button" variant="outline" onClick={onCancel}>
							Cancel
						</Button>
					</DialogFooter>
				</>
			)}
		</div>
	)
}

function ZenSetupView({
	provider,
	state,
	onSubmit,
	onCancel,
}: {
	provider: CatalogProvider
	state: DialogState
	onSubmit: (apiKey: string) => void
	onCancel: () => void
}) {
	const [apiKey, setApiKey] = useState("")
	const isLoading = state.status === "loading"

	const models = provider.models ?? {}
	const freeModelCount = Object.values(models).filter(
		(m) => (m as { cost?: { input?: number } }).cost?.input === 0,
	).length
	const totalModelCount = Object.keys(models).length

	const handleSubmit = useCallback(
		(e: React.FormEvent) => {
			e.preventDefault()
			if (apiKey.trim()) {
				onSubmit(apiKey.trim())
			}
		},
		[apiKey, onSubmit],
	)

	return (
		<form onSubmit={handleSubmit}>
			<div className="space-y-4 py-2">
				{/* Free tier info */}
				<div className="flex items-start gap-3 rounded-lg bg-emerald-500/5 border border-emerald-500/20 px-4 py-3">
					<ZapIcon className="mt-0.5 size-4 shrink-0 text-emerald-500" aria-hidden="true" />
					<div className="space-y-0.5 text-sm">
						<p className="font-medium text-emerald-700 dark:text-emerald-400">
							{freeModelCount} free models included
						</p>
						<p className="text-xs text-muted-foreground">
							You can start coding immediately. Add an API key to unlock {totalModelCount}+ premium
							models from Claude, GPT, Gemini, and more.
						</p>
					</div>
				</div>

				{/* API key input */}
				<div className="space-y-2">
					<Label htmlFor="zen-api-key">API Key</Label>
					<Input
						id="zen-api-key"
						type="password"
						placeholder="sk-..."
						value={apiKey}
						onChange={(e) => setApiKey(e.target.value)}
						disabled={isLoading}
						autoFocus
					/>
					<a
						href={ZEN_SIGNUP_URL}
						target="_blank"
						rel="noopener noreferrer"
						className="inline-flex items-center gap-1.5 text-xs text-primary hover:underline"
					>
						<SparklesIcon className="size-3" aria-hidden="true" />
						Get an API key at devo.ai/zen
						<ExternalLinkIcon className="size-2.5" aria-hidden="true" />
					</a>
				</div>

				{state.status === "error" && (
					<div className="flex items-center gap-2 text-sm text-destructive">
						<AlertCircleIcon className="size-4 shrink-0" aria-hidden="true" />
						<span>{state.message}</span>
					</div>
				)}
			</div>

			<DialogFooter className="mt-4">
				<Button type="button" variant="outline" onClick={onCancel} disabled={isLoading}>
					{apiKey.trim() ? "Cancel" : "Use free models"}
				</Button>
				<Button type="submit" disabled={!apiKey.trim() || isLoading}>
					{isLoading ? (
						<>
							<Spinner className="size-4" />
							Connecting...
						</>
					) : (
						"Connect"
					)}
				</Button>
			</DialogFooter>
		</form>
	)
}

function SuccessView({ provider, onDone }: { provider: CatalogProvider; onDone: () => void }) {
	return (
		<>
			<div className="flex flex-col items-center gap-3 py-6">
				<CheckCircle2Icon className="size-8 text-green-500" aria-hidden="true" />
				<p className="text-sm font-medium">Connected to {provider.name}</p>
				<p className="text-xs text-muted-foreground">
					You can now use {provider.name} models in your conversations
				</p>
			</div>
			<DialogFooter>
				<Button onClick={onDone}>Done</Button>
			</DialogFooter>
		</>
	)
}

// ============================================================
// Helpers
// ============================================================

async function pollForCompletion(
	client: ReturnType<typeof getBaseClient>,
	providerID: string,
	methodIndex: number,
	setState: (state: DialogState) => void,
	onSuccess: () => void,
	isCancelled: () => boolean,
) {
	if (!client) return

	const maxAttempts = 60
	const intervalMs = 2000

	for (let attempt = 0; attempt < maxAttempts; attempt++) {
		if (isCancelled()) return

		await new Promise((resolve) => setTimeout(resolve, intervalMs))
		if (isCancelled()) return

		try {
			await client.provider.oauth.callback({
				providerID,
				method: methodIndex,
			})
			// If no error, OAuth completed
			if (isCancelled()) return
			await client.global.dispose()
			onSuccess()
			return
		} catch {
			// Expected to fail while user hasn't completed OAuth yet
		}
	}

	if (!isCancelled()) {
		setState({ status: "error", message: "Authentication timed out. Please try again." })
	}
}

function extractDeviceCode(instructions: string | null): string | null {
	if (!instructions) return null
	const match = instructions.match(/\b[A-Z0-9]{4}(?:[- ][A-Z0-9]{4})+\b/i)
	if (!match) return null
	return match[0].toUpperCase().replace(/\s+/g, "-")
}
