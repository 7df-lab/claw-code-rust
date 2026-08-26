import type {
	DevoClient,
	ProviderValidateParams,
	ProviderVendor,
	ProviderVendorUpsertParams,
	ProviderWireApi,
} from "@devo-ai/sdk/v2/client"
import { Alert, AlertDescription } from "@devo/ui/components/alert"
import { Button } from "@devo/ui/components/button"
import { Checkbox } from "@devo/ui/components/checkbox"
import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogFooter,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import {
	Field,
	FieldContent,
	FieldDescription,
	FieldError,
	FieldGroup,
	FieldLabel,
} from "@devo/ui/components/field"
import { Input } from "@devo/ui/components/input"
import {
	Select,
	SelectContent,
	SelectGroup,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { ScrollArea } from "@devo/ui/components/scroll-area"
import { Spinner } from "@devo/ui/components/spinner"
import { Textarea } from "@devo/ui/components/textarea"
import { useQueryClient } from "@tanstack/react-query"
import { SaveIcon } from "lucide-react"
import { useCallback, useEffect, useRef, useState } from "react"
import { queryKeys } from "../../hooks/use-devo-data"
import { createLogger } from "../../lib/logger"
import { getBaseClient, invalidateConfigOptionCaches } from "../../services/connection-manager"

const log = createLogger("provider-vendor-dialog")

const WIRE_API_OPTIONS: Array<{ value: ProviderWireApi; label: string }> = [
	{ value: "openai_chat_completions", label: "OpenAI Chat Completions" },
	{ value: "openai_responses", label: "OpenAI Responses" },
	{ value: "anthropic_messages", label: "Anthropic Messages" },
]

export interface ProviderVendorFormValues {
	providerName: string
	baseUrl: string
	wireApi: ProviderWireApi
	enabled: boolean
	headers: string
	apiKey: string
	bindingId: string
	modelSlug: string
	requestModel: string
	displayName: string
	defaultReasoningEffort: string
	makeDefault: boolean
}

interface ProviderVendorClient {
	provider: Pick<DevoClient["provider"], "validate" | "upsert">
}

interface ProviderVendorDialogProps {
	providerVendor: ProviderVendor | null
	open: boolean
	onOpenChange: (open: boolean) => void
	onSaved: () => void
}

function trimToNull(value: string): string | null {
	const trimmed = value.trim()
	return trimmed.length > 0 ? trimmed : null
}

function credentialIdForProvider(providerName: string): string {
	const normalized = providerName
		.trim()
		.toLowerCase()
		.replace(/[^a-z0-9]+/g, "_")
		.replace(/^_+|_+$/g, "")
	return `${normalized || "provider"}_api_key`
}

function normalizeHeaders(value: string): string | null {
	const trimmed = value.trim()
	if (!trimmed) return null
	let parsed: unknown
	try {
		parsed = JSON.parse(trimmed)
	} catch {
		throw new Error("Headers must be a JSON object")
	}
	if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
		throw new Error("Headers must be a JSON object")
	}
	return trimmed
}

function initialValues(providerVendor: ProviderVendor | null): ProviderVendorFormValues {
	const providerName = providerVendor?.name ?? ""
	const wireApi = providerVendor?.wire_apis[0] ?? "openai_chat_completions"
	return {
		providerName,
		baseUrl: providerVendor?.base_url ?? "",
		wireApi,
		enabled: providerVendor?.enabled ?? true,
		headers: providerVendor?.headers ?? "",
		apiKey: "",
		bindingId: providerName ? `${providerName}-model` : "",
		modelSlug: "",
		requestModel: "",
		displayName: "",
		defaultReasoningEffort: "",
		makeDefault: false,
	}
}

function requireText(value: string, label: string): string {
	const trimmed = value.trim()
	if (!trimmed) throw new Error(`${label} is required`)
	return trimmed
}

export function buildProviderUpsertParams(
	values: ProviderVendorFormValues,
	existingProvider: ProviderVendor | null,
): ProviderVendorUpsertParams {
	const providerName = requireText(values.providerName, "Provider name")
	const bindingId = requireText(values.bindingId, "Model binding id")
	const modelSlug = requireText(values.modelSlug, "Model slug")
	const requestModel = requireText(values.requestModel, "Request model name")
	const apiKey = trimToNull(values.apiKey)
	const credential =
		existingProvider?.credential ?? (apiKey ? credentialIdForProvider(providerName) : null)

	return {
		provider_vendor: {
			name: providerName,
			base_url: trimToNull(values.baseUrl),
			credential,
			headers: normalizeHeaders(values.headers),
			wire_apis: [values.wireApi],
			enabled: values.enabled,
		},
		model_binding: {
			binding_id: bindingId,
			model_slug: modelSlug,
			provider: providerName,
			request_model: requestModel,
			display_name: trimToNull(values.displayName),
			invocation_method: values.wireApi,
			default_reasoning_effort: trimToNull(values.defaultReasoningEffort),
			enabled: values.enabled,
		},
		default_model_binding: values.makeDefault ? bindingId : null,
		...(apiKey ? { api_key: apiKey } : {}),
	}
}

export async function saveProviderVendor(
	client: ProviderVendorClient,
	params: ProviderVendorUpsertParams,
	shouldContinue: () => boolean = () => true,
) {
	if (!params.model_binding) {
		throw new Error("Model binding is required")
	}
	if (!shouldContinue()) return
	const validateParams: ProviderValidateParams = {
		provider_vendor: params.provider_vendor,
		model_binding: params.model_binding,
		...(params.api_key ? { api_key: params.api_key } : {}),
	}
	await client.provider.validate(validateParams)
	if (!shouldContinue()) return
	return client.provider.upsert(params)
}

export function ProviderVendorDialog({
	providerVendor,
	open,
	onOpenChange,
	onSaved,
}: ProviderVendorDialogProps) {
	const queryClient = useQueryClient()
	const [values, setValues] = useState<ProviderVendorFormValues>(() => initialValues(providerVendor))
	const [error, setError] = useState<string | null>(null)
	const [saving, setSaving] = useState(false)
	const saveAttemptRef = useRef(0)

	useEffect(() => {
		saveAttemptRef.current += 1
		if (!open) return
		setValues(initialValues(providerVendor))
		setError(null)
		setSaving(false)
	}, [open, providerVendor])

	const setValue = useCallback(
		<K extends keyof ProviderVendorFormValues>(key: K, value: ProviderVendorFormValues[K]) => {
			setValues((current) => ({ ...current, [key]: value }))
		},
		[],
	)

	const handleSubmit = useCallback(
		async (event: React.FormEvent<HTMLFormElement>) => {
			event.preventDefault()
			const saveAttempt = ++saveAttemptRef.current
			setSaving(true)
			setError(null)
			try {
				const client = getBaseClient()
				if (!client) throw new Error("Not connected to server")
				const params = buildProviderUpsertParams(values, providerVendor)
				await saveProviderVendor(client, params, () => saveAttemptRef.current === saveAttempt)
				if (saveAttemptRef.current !== saveAttempt) return
				invalidateConfigOptionCaches()
				queryClient.invalidateQueries({ queryKey: queryKeys.providerVendors })
				queryClient.invalidateQueries({
					predicate: (query) => query.queryKey[0] === "providers" || query.queryKey[0] === "config",
				})
				onSaved()
				onOpenChange(false)
			} catch (err) {
				if (saveAttemptRef.current !== saveAttempt) return
				const message = err instanceof Error ? err.message : "Failed to save provider"
				log.error("Failed to save provider", { error: err })
				setError(message)
			} finally {
				if (saveAttemptRef.current === saveAttempt) setSaving(false)
			}
		},
		[values, providerVendor, queryClient, onSaved, onOpenChange],
	)

	const handleDialogOpenChange = useCallback(
		(nextOpen: boolean) => {
			if (!nextOpen) {
				// Closing while validation is pending invalidates the continuation so
				// a late validation response cannot persist the provider.
				saveAttemptRef.current += 1
				setSaving(false)
			}
			onOpenChange(nextOpen)
		},
		[onOpenChange],
	)

	return (
		<Dialog open={open} onOpenChange={handleDialogOpenChange}>
			<DialogContent className="flex max-h-[calc(100dvh-2rem)] overflow-hidden p-0 sm:max-w-xl">
				<form onSubmit={handleSubmit} className="flex min-h-0 flex-col">
					<DialogHeader className="px-6 pt-6 pb-4">
						<DialogTitle>{providerVendor ? "Edit Provider" : "Add Provider"}</DialogTitle>
						<DialogDescription>
							Configure the provider endpoint, credential, and first model binding.
						</DialogDescription>
					</DialogHeader>

					<ScrollArea
						className="min-h-0 flex-1 border-y"
						data-testid="provider-dialog-scroll-body"
					>
						<div className="px-6 py-4">
							{error && (
								<Alert variant="destructive" className="mb-4">
									<AlertDescription>{error}</AlertDescription>
								</Alert>
							)}

							<FieldGroup className="gap-4">
								<div className="grid gap-4 sm:grid-cols-2">
									<TextField
										id="provider-name"
										label="Provider name"
										value={values.providerName}
										onChange={(value) => setValue("providerName", value)}
										placeholder="openai"
										disabled={saving}
									/>
									<Field>
										<FieldLabel htmlFor="wire-api">Wire API</FieldLabel>
										<Select
											value={values.wireApi}
											onValueChange={(value) => setValue("wireApi", value as ProviderWireApi)}
											disabled={saving}
										>
											<SelectTrigger id="wire-api" className="w-full">
												<SelectValue placeholder="Select wire API" />
											</SelectTrigger>
											<SelectContent align="start" alignItemWithTrigger={false}>
												<SelectGroup>
													{WIRE_API_OPTIONS.map((option) => (
														<SelectItem key={option.value} value={option.value}>
															{option.label}
														</SelectItem>
													))}
												</SelectGroup>
											</SelectContent>
										</Select>
									</Field>
								</div>

								<TextField
									id="base-url"
									label="Base URL"
									value={values.baseUrl}
									onChange={(value) => setValue("baseUrl", value)}
									placeholder="https://api.openai.com/v1"
									disabled={saving}
									optional
								/>

								<TextField
									id="api-key"
									label="API key"
									value={values.apiKey}
									onChange={(value) => setValue("apiKey", value)}
									placeholder={providerVendor?.credential ? "Leave blank to keep existing key" : "sk-..."}
									disabled={saving}
									type="password"
									optional={!!providerVendor?.credential}
								/>

								<Field>
									<FieldLabel htmlFor="headers">Custom headers</FieldLabel>
									<Textarea
										id="headers"
										value={values.headers}
										onChange={(event) => setValue("headers", event.target.value)}
										placeholder='{"HTTP-Referer":"https://devo.ai"}'
										disabled={saving}
									/>
									<FieldDescription>Optional JSON object.</FieldDescription>
								</Field>

								<div className="grid gap-4 sm:grid-cols-2">
									<TextField
										id="binding-id"
										label="Model binding id"
										value={values.bindingId}
										onChange={(value) => setValue("bindingId", value)}
										placeholder="openai-gpt-4o"
										disabled={saving}
									/>
									<TextField
										id="model-slug"
										label="Model slug"
										value={values.modelSlug}
										onChange={(value) => setValue("modelSlug", value)}
										placeholder="gpt-4o"
										disabled={saving}
									/>
								</div>

								<div className="grid gap-4 sm:grid-cols-2">
									<TextField
										id="request-model"
										label="Request model name"
										value={values.requestModel}
										onChange={(value) => setValue("requestModel", value)}
										placeholder="gpt-4o"
										disabled={saving}
									/>
									<TextField
										id="display-name"
										label="Display name"
										value={values.displayName}
										onChange={(value) => setValue("displayName", value)}
										placeholder="GPT-4o"
										disabled={saving}
										optional
									/>
								</div>

								<TextField
									id="reasoning-effort"
									label="Default reasoning effort"
									value={values.defaultReasoningEffort}
									onChange={(value) => setValue("defaultReasoningEffort", value)}
									placeholder="medium"
									disabled={saving}
									optional
								/>

								<div className="grid gap-3 sm:grid-cols-2">
									<CheckboxField
										id="provider-enabled"
										label="Enabled"
										description="Provider and model binding are available for selection."
										checked={values.enabled}
										disabled={saving}
										onCheckedChange={(checked) => setValue("enabled", checked)}
									/>
									<CheckboxField
										id="make-default"
										label="Make default"
										description="Set this model binding as the default model."
										checked={values.makeDefault}
										disabled={saving}
										onCheckedChange={(checked) => setValue("makeDefault", checked)}
									/>
								</div>
							</FieldGroup>
						</div>
					</ScrollArea>

					<DialogFooter
						className="shrink-0 bg-background px-6 py-4"
						data-testid="provider-dialog-footer"
					>
						<Button type="button" variant="outline" onClick={() => handleDialogOpenChange(false)}>
							Cancel
						</Button>
						<Button type="submit" disabled={saving}>
							{saving ? <Spinner /> : <SaveIcon data-icon="inline-start" />}
							Validate & Save
						</Button>
					</DialogFooter>
				</form>
			</DialogContent>
		</Dialog>
	)
}

function TextField({
	id,
	label,
	value,
	onChange,
	placeholder,
	disabled,
	type = "text",
	optional = false,
}: {
	id: string
	label: string
	value: string
	onChange: (value: string) => void
	placeholder: string
	disabled: boolean
	type?: string
	optional?: boolean
}) {
	return (
		<Field>
			<FieldLabel htmlFor={id}>
				{label}
				{optional && <span className="text-xs font-normal text-muted-foreground">(optional)</span>}
			</FieldLabel>
			<Input
				id={id}
				type={type}
				value={value}
				onChange={(event) => onChange(event.target.value)}
				placeholder={placeholder}
				disabled={disabled}
			/>
		</Field>
	)
}

function CheckboxField({
	id,
	label,
	description,
	checked,
	disabled,
	onCheckedChange,
}: {
	id: string
	label: string
	description: string
	checked: boolean
	disabled: boolean
	onCheckedChange: (checked: boolean) => void
}) {
	return (
		<Field orientation="horizontal">
			<Checkbox
				id={id}
				checked={checked}
				onCheckedChange={(value) => onCheckedChange(value === true)}
				disabled={disabled}
			/>
			<FieldContent>
				<FieldLabel htmlFor={id}>{label}</FieldLabel>
				<FieldDescription>{description}</FieldDescription>
				<FieldError />
			</FieldContent>
		</Field>
	)
}
