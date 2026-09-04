/**
 * Dialog for creating or editing a custom provider Connection.
 *
 * All fields are user-editable: id, name, base URL, wire API, API key,
 * models, and optional request headers.
 */

import type { CatalogModelInfo, CatalogProviderInfo, CatalogWireApi } from "@devo-ai/sdk/v2/client"
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
import {
	Select,
	SelectContent,
	SelectGroup,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { Spinner } from "@devo/ui/components/spinner"
import { PlusIcon, Trash2Icon } from "lucide-react"
import { useCallback, useState } from "react"
import { getBaseClient } from "../../services/connection-manager"

const WIRE_API_OPTIONS: Array<{ value: CatalogWireApi; label: string }> = [
	{ value: "openai_chat_completions", label: "OpenAI Chat Completions" },
	{ value: "openai_responses", label: "OpenAI Responses" },
	{ value: "anthropic_messages", label: "Anthropic Messages" },
]

interface CustomProviderDialogProps {
	/** Pass an existing provider to edit; omit for new. */
	provider?: CatalogProviderInfo
	open: boolean
	onOpenChange: (open: boolean) => void
	onSaved: () => void
}

interface ModelEntry {
	id: string
	name: string
}

interface HeaderEntry {
	key: string
	value: string
}

export function CustomProviderDialog({
	provider: existing,
	open,
	onOpenChange,
	onSaved,
}: CustomProviderDialogProps) {
	const isEdit = !!existing

	const [providerId, setProviderId] = useState(existing?.id ?? "")
	const [displayName, setDisplayName] = useState(existing?.name ?? "")
	const [baseUrl, setBaseUrl] = useState(existing?.baseUrl ?? "")
	const [wireApi, setWireApi] = useState<CatalogWireApi>(
		existing?.wireApis[0] ?? "openai_chat_completions",
	)
	const [apiKey, setApiKey] = useState("")
	const [models, setModels] = useState<ModelEntry[]>(() => {
		if (!existing) return [{ id: "", name: "" }]
		return Object.entries(existing.models ?? {}).map(([id, m]) => ({
			id,
			name: m.name ?? "",
		}))
	})
	const [headers, setHeaders] = useState<HeaderEntry[]>(() => {
		if (!existing?.headers) return []
		return Object.entries(existing.headers).map(([key, value]) => ({ key, value }))
	})
	const [saving, setSaving] = useState(false)
	const [error, setError] = useState<string | null>(null)

	const addModel = useCallback(() => {
		setModels((prev) => [...prev, { id: "", name: "" }])
	}, [])

	const removeModel = useCallback((index: number) => {
		setModels((prev) => prev.filter((_, i) => i !== index))
	}, [])

	const updateModel = useCallback((index: number, field: "id" | "name", value: string) => {
		setModels((prev) => prev.map((m, i) => (i === index ? { ...m, [field]: value } : m)))
	}, [])

	const addHeader = useCallback(() => {
		setHeaders((prev) => [...prev, { key: "", value: "" }])
	}, [])

	const removeHeader = useCallback((index: number) => {
		setHeaders((prev) => prev.filter((_, i) => i !== index))
	}, [])

	const updateHeader = useCallback((index: number, field: "key" | "value", value: string) => {
		setHeaders((prev) => prev.map((h, i) => (i === index ? { ...h, [field]: value } : h)))
	}, [])

	const handleSave = useCallback(async () => {
		const id = providerId.trim()
		if (!id) {
			setError("Provider ID is required")
			return
		}
		if (!baseUrl.trim()) {
			setError("Base URL is required")
			return
		}

		setSaving(true)
		setError(null)
		try {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")

			const catalogModels: Record<string, CatalogModelInfo> = {}
			for (const m of models) {
				const mid = m.id.trim()
				if (mid) {
					catalogModels[mid] = { name: m.name.trim() || undefined }
				}
			}

			const catalogHeaders: Record<string, string> = {}
			for (const h of headers) {
				if (h.key.trim()) {
					catalogHeaders[h.key.trim()] = h.value
				}
			}

			const provider: CatalogProviderInfo = {
				id,
				name: displayName.trim() || id,
				baseUrl: baseUrl.trim(),
				wireApis: [wireApi],
				models: catalogModels,
				enabled: true,
				...(Object.keys(catalogHeaders).length > 0 ? { headers: catalogHeaders } : {}),
			}

			await client.provider.upsert({
				provider,
				...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
			})

			onOpenChange(false)
			onSaved()
		} catch (err) {
			setError(err instanceof Error ? err.message : "Failed to save provider")
		} finally {
			setSaving(false)
		}
	}, [providerId, displayName, baseUrl, wireApi, apiKey, models, headers, onOpenChange, onSaved])

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="sm:max-w-2xl max-h-[85vh] overflow-y-auto">
				<DialogHeader>
					<DialogTitle>{isEdit ? "Edit Custom Provider" : "Add Custom Provider"}</DialogTitle>
					<DialogDescription>
						Configure an OpenAI-compatible or Anthropic-compatible provider.
					</DialogDescription>
				</DialogHeader>

				<div className="flex flex-col gap-4 py-2">
					{/* Provider ID */}
					<div className="flex flex-col gap-1.5">
						<Label htmlFor="provider-id" className="text-[13px]">
							Provider ID
						</Label>
						<Input
							id="provider-id"
							placeholder="my-provider"
							value={providerId}
							onChange={(e) => setProviderId(e.target.value)}
							disabled={isEdit || saving}
						/>
						<p className="text-[12px] text-muted-foreground">
							Lowercase letters, numbers, hyphens, or underscores.
						</p>
					</div>

					{/* Display name */}
					<div className="flex flex-col gap-1.5">
						<Label htmlFor="display-name" className="text-[13px]">
							Display Name
						</Label>
						<Input
							id="display-name"
							placeholder={providerId || "My Provider"}
							value={displayName}
							onChange={(e) => setDisplayName(e.target.value)}
							disabled={saving}
						/>
					</div>

					{/* Base URL */}
					<div className="flex flex-col gap-1.5">
						<Label htmlFor="base-url" className="text-[13px]">
							Base URL
						</Label>
						<Input
							id="base-url"
							placeholder="https://api.example.com/v1"
							value={baseUrl}
							onChange={(e) => setBaseUrl(e.target.value)}
							disabled={saving}
						/>
					</div>

					{/* Wire API */}
					<div className="flex flex-col gap-1.5">
						<Label className="text-[13px]">Wire API</Label>
						<Select value={wireApi} onValueChange={(v) => setWireApi(v as CatalogWireApi)}>
							<SelectTrigger>
								<SelectValue />
							</SelectTrigger>
							<SelectContent>
								<SelectGroup>
									{WIRE_API_OPTIONS.map((opt) => (
										<SelectItem key={opt.value} value={opt.value}>
											{opt.label}
										</SelectItem>
									))}
								</SelectGroup>
							</SelectContent>
						</Select>
					</div>

					{/* API key */}
					<div className="flex flex-col gap-1.5">
						<Label htmlFor="api-key" className="text-[13px]">
							API Key
						</Label>
						<Input
							id="api-key"
							type="password"
							placeholder={isEdit ? "(unchanged)" : "sk-…"}
							value={apiKey}
							onChange={(e) => setApiKey(e.target.value)}
							disabled={saving}
						/>
						<p className="text-[12px] text-muted-foreground">
							Optional if you authenticate via request headers.
						</p>
					</div>

					{/* Models */}
					<div className="flex flex-col gap-2">
						<Label className="text-[13px]">Models</Label>
						{models.map((m, i) => (
							<div key={i} className="flex items-center gap-2">
								<Input
									placeholder="model-id"
									value={m.id}
									onChange={(e) => updateModel(i, "id", e.target.value)}
									className="flex-1"
									disabled={saving}
								/>
								<Input
									placeholder="Display name"
									value={m.name}
									onChange={(e) => updateModel(i, "name", e.target.value)}
									className="flex-1"
									disabled={saving}
								/>
								<Button
									variant="ghost"
									size="sm"
									onClick={() => removeModel(i)}
									disabled={saving || models.length <= 1}
								>
									<Trash2Icon className="size-3.5 stroke-[1.5]" />
								</Button>
							</div>
						))}
						<Button
							variant="ghost"
							size="sm"
							className="self-start text-[13px]"
							onClick={addModel}
							disabled={saving}
						>
							<PlusIcon className="size-3.5 stroke-[1.5]" />
							Add model
						</Button>
					</div>

					{/* Headers */}
					<div className="flex flex-col gap-2">
						<Label className="text-[13px]">Request Headers (optional)</Label>
						{headers.map((h, i) => (
							<div key={i} className="flex items-center gap-2">
								<Input
									placeholder="Header-Name"
									value={h.key}
									onChange={(e) => updateHeader(i, "key", e.target.value)}
									className="flex-1"
									disabled={saving}
								/>
								<Input
									placeholder="value"
									value={h.value}
									onChange={(e) => updateHeader(i, "value", e.target.value)}
									className="flex-1"
									disabled={saving}
								/>
								<Button variant="ghost" size="sm" onClick={() => removeHeader(i)} disabled={saving}>
									<Trash2Icon className="size-3.5 stroke-[1.5]" />
								</Button>
							</div>
						))}
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

					{error && <p className="text-[13px] text-destructive">{error}</p>}
				</div>

				<DialogFooter>
					<Button variant="outline" onClick={() => onOpenChange(false)} disabled={saving}>
						Cancel
					</Button>
					<Button onClick={handleSave} disabled={saving}>
						{saving && <Spinner className="size-3.5" />}
						{saving ? "Saving…" : isEdit ? "Save" : "Add Provider"}
					</Button>
				</DialogFooter>
			</DialogContent>
		</Dialog>
	)
}
