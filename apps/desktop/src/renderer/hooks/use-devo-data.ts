import type {
	Agent as SdkAgent,
	Command as SdkCommand,
	Config as SdkConfig,
	Model as SdkModel,
	Provider as SdkProvider,
	ProviderAuthMethod as SdkProviderAuthMethod,
	ProviderVendor as SdkProviderVendor,
} from "@devo-ai/sdk/v2/client"
import { useQuery, useQueryClient } from "@tanstack/react-query"
import { useAtomValue } from "jotai"
import { useCallback } from "react"
import { serverConnectedAtom } from "../atoms/connection"
import { isMockModeAtom } from "../atoms/mock-mode"
import { MOCK_AGENTS, MOCK_CONFIG, MOCK_PROVIDERS } from "../lib/mock-data"
import type { GitBranchInfo, GitBranchState } from "../../preload/api"
import { fetchGitBranches, fetchModelState, isElectron, updateModelRecent } from "../services/backend"
import { getBaseClient, getProjectClient } from "../services/connection-manager"

// ============================================================
// Re-exports — use SDK types directly
// ============================================================

export type { SdkAgent, SdkCommand, SdkConfig, SdkModel, SdkProvider, SdkProviderAuthMethod }
export type { SdkProviderVendor }

// ============================================================
// Derived types for our UI layer
// ============================================================

export interface ProvidersData {
	providers: SdkProvider[]
	defaults: Record<string, string>
}

export interface VcsData {
	branch: string
	state: GitBranchState
	detached: boolean
}

export interface CompactionConfig {
	/** Whether automatic compaction is enabled (default: true) */
	auto?: boolean
	/** Token buffer reserved for compaction (default: 20,000) */
	reserved?: number
}

export interface ConfigData {
	model?: string
	smallModel?: string
	defaultAgent?: string
	compaction?: CompactionConfig
}

export interface ModelRef {
	providerID: string
	modelID: string
}

// ============================================================
// Helpers
// ============================================================

export function parseModelRef(ref: string): ModelRef | null {
	const slashIndex = ref.indexOf("/")
	if (slashIndex === -1) return null
	return {
		providerID: ref.slice(0, slashIndex),
		modelID: ref.slice(slashIndex + 1),
	}
}

export function getModelDisplayName(modelID: string, providers: SdkProvider[]): string {
	for (const provider of providers) {
		const model = provider.models[modelID]
		if (model) return model.name
	}
	return modelID
		.replace(/-\d{8}$/, "")
		.replace(/-/g, " ")
		.replace(/\b\w/g, (c) => c.toUpperCase())
}

export function getModelVariants(
	providerID: string,
	modelID: string,
	providers: SdkProvider[],
): string[] {
	for (const provider of providers) {
		if (provider.id !== providerID) continue
		const model = provider.models[modelID]
		if (model?.variants) {
			return Object.keys(model.variants)
		}
	}
	return []
}

export function getModelCurrentVariant(
	providerID: string,
	modelID: string,
	providers: SdkProvider[],
): string | undefined {
	for (const provider of providers) {
		if (provider.id !== providerID) continue
		const model = provider.models[modelID] as { currentVariant?: unknown; variants?: unknown }
		const currentVariant = model?.currentVariant
		if (typeof currentVariant !== "string") return undefined
		const variants = model?.variants && typeof model.variants === "object" ? model.variants : {}
		return currentVariant in variants ? currentVariant : undefined
	}
	return undefined
}

export function modelAllowsDefaultVariant(
	providerID: string,
	modelID: string,
	providers: SdkProvider[],
): boolean {
	for (const provider of providers) {
		if (provider.id !== providerID) continue
		const model = provider.models[modelID] as { allowDefaultVariant?: unknown }
		return model?.allowDefaultVariant !== false
	}
	return true
}

/**
 * Reverse lookup: which provider owns a bare model id (slug). Persisted
 * sessions store only the model slug — the provider binding may be "unknown"
 * on cold-restored sessions — but the provider list keyed by model id is
 * enough to rebuild a full ModelRef for the composer.
 *
 * Sessions whose last state came from a turn report the RESOLVED request
 * slug, so when the exact id misses, fall back to the longest known model id
 * at a dash boundary on either side: `<config-id>-<provider-name>` (e.g.
 * `deepseek-v4-flash-deepseek-ac`) and provider-prefixed ids (e.g.
 * `ollama-qwen3.6-35b-a3b`) both resolve back to the configured model.
 */
export function modelRefFromSlug(
	modelID: string,
	providers: SdkProvider[],
): ModelRef | null {
	if (!modelID) return null
	for (const provider of providers) {
		if (provider?.models && provider.models[modelID]) {
			return { providerID: provider.id, modelID }
		}
	}
	let best: { providerID: string; modelID: string } | null = null
	for (const provider of providers) {
		for (const candidate of Object.keys(provider?.models ?? {})) {
			if (
				candidate.length > (best?.modelID.length ?? 0) &&
				(modelID === candidate ||
					modelID.startsWith(`${candidate}-`) ||
					modelID.endsWith(`-${candidate}`))
			) {
				best = { providerID: provider.id, modelID: candidate }
			}
		}
	}
	return best
}

export function resolveEffectiveModel(
	selectedModel: ModelRef | null,
	agent: SdkAgent | null,
	configModel: string | undefined,
	providerDefaults: Record<string, string>,
	providers: SdkProvider[],
	recentModels?: ModelRef[],
): ModelRef | null {
	if (selectedModel) return selectedModel
	if (agent?.model) {
		return { providerID: agent.model.providerID, modelID: agent.model.modelID }
	}
	if (configModel) {
		const ref = parseModelRef(configModel)
		// Skip a preference the provider list cannot serve — historically the
		// preference could be poisoned with resolved request slugs, and using
		// them here would re-propagate the slug into every fallback consumer.
		if (ref && providers.some((p) => p.models[ref.modelID])) return ref
	}
	if (recentModels) {
		for (const recent of recentModels) {
			const provider = providers.find((p) => p.id === recent.providerID)
			if (provider?.models[recent.modelID]) {
				return recent
			}
		}
	}
	for (const provider of providers) {
		const defaultModelId = providerDefaults[provider.id]
		if (defaultModelId) {
			return { providerID: provider.id, modelID: defaultModelId }
		}
	}
	return null
}

export function getModelInputCapabilities(
	model: ModelRef | null,
	providers: SdkProvider[],
): { image: boolean; pdf: boolean; attachment: boolean } | null {
	if (!model) return null
	for (const provider of providers) {
		if (provider.id !== model.providerID) continue
		const m = provider.models[model.modelID]
		if (m?.capabilities) {
			return {
				image: m.capabilities.input.image,
				pdf: m.capabilities.input.pdf,
				attachment: m.capabilities.attachment,
			}
		}
	}
	return null
}

interface LoadVcsDataOptions {
	isElectron?: boolean
	fetchBranches?: (directory: string) => Promise<GitBranchInfo>
	getClient?: (directory: string) => { vcs: { get: () => Promise<{ data?: unknown }> } } | null
}

export async function loadVcsData(
	directory: string,
	options: LoadVcsDataOptions = {},
): Promise<VcsData> {
	if (options.isElectron ?? isElectron) {
		const branches = await (options.fetchBranches ?? fetchGitBranches)(directory)
		return {
			branch: branches.current ?? "",
			state: branches.state,
			detached: branches.detached,
		}
	}

	const client = (options.getClient ?? getProjectClient)(directory)
	if (!client) throw new Error("No client for directory")
	const result = await client.vcs.get()
	const raw = result.data as
		| { branch?: string; state?: GitBranchState; detached?: boolean }
		| null
		| undefined
	const branch = raw?.branch ?? ""
	return {
		branch,
		state: raw?.state ?? (branch ? "branch" : "not_git"),
		detached: raw?.detached ?? false,
	}
}

// ============================================================
// Query Key Factories
// ============================================================

export const queryKeys = {
	providers: (directory: string) => ["providers", directory] as const,
	config: (directory: string) => ["config", directory] as const,
	vcs: (directory: string) => ["vcs", directory] as const,
	agents: (directory: string) => ["agents", directory] as const,
	commands: (directory: string) => ["commands", directory] as const,
	modelState: ["modelState"] as const,
	allProviders: ["allProviders"] as const,
	connectedProviders: ["connectedProviders"] as const,
	providerVendors: ["providerVendors"] as const,
	providerAuthMethods: ["providerAuthMethods"] as const,
}

// ============================================================
// Hooks (TanStack Query)
// ============================================================

export function useProviders(directory: string | null): {
	data: ProvidersData | null
	loading: boolean
	error: string | null
	reload: () => void
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.providers(directory ?? ""),
		queryFn: async (): Promise<ProvidersData> => {
			const client = getProjectClient(directory!)
			if (!client) throw new Error("No client for directory")
			const result = await client.config.providers()
			const raw = result.data as {
				providers: SdkProvider[]
				default: Record<string, string>
			}
			return {
				providers: raw.providers ?? [],
				defaults: raw.default ?? {},
			}
		},
		enabled: !!directory && connected && !isMockMode,
	})

	const reload = useCallback(() => {
		if (directory) {
			queryClient.invalidateQueries({ queryKey: queryKeys.providers(directory) })
		}
	}, [directory, queryClient])

	// Return mock data if in mock mode
	if (isMockMode && directory) {
		return {
			data: MOCK_PROVIDERS as unknown as ProvidersData,
			loading: false,
			error: null,
			reload,
		}
	}

	return {
		data: data ?? null,
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load providers") : null,
		reload,
	}
}

export function useConfig(directory: string | null): {
	data: ConfigData | null
	loading: boolean
	error: string | null
	reload: () => void
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.config(directory ?? ""),
		queryFn: async (): Promise<ConfigData> => {
			const client = getProjectClient(directory!)
			if (!client) throw new Error("No client for directory")
			const result = await client.config.get()
			const raw = result.data as SdkConfig
			return {
				model: raw.model,
				smallModel: raw.small_model,
				defaultAgent: raw.default_agent,
				compaction: raw.compaction
					? { auto: raw.compaction.auto, reserved: raw.compaction.reserved }
					: undefined,
			}
		},
		enabled: !!directory && connected && !isMockMode,
	})

	const reload = useCallback(() => {
		if (directory) {
			queryClient.invalidateQueries({ queryKey: queryKeys.config(directory) })
		}
	}, [directory, queryClient])

	// Return mock data if in mock mode
	if (isMockMode && directory) {
		return {
			data: MOCK_CONFIG,
			loading: false,
			error: null,
			reload,
		}
	}

	return {
		data: data ?? null,
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load config") : null,
		reload,
	}
}

export function useVcs(directory: string | null): {
	data: VcsData | null
	loading: boolean
	error: string | null
	reload: () => void
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.vcs(directory ?? ""),
		queryFn: async (): Promise<VcsData> => loadVcsData(directory!),
		enabled: !!directory && connected && !isMockMode,
		staleTime: 30_000,
		refetchInterval: 60_000,
	})

	const reload = useCallback(() => {
		if (directory) {
			queryClient.invalidateQueries({ queryKey: queryKeys.vcs(directory) })
		}
	}, [directory, queryClient])

	return {
		data: data ?? null,
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load VCS info") : null,
		reload,
	}
}

export function useDevoAgents(directory: string | null): {
	agents: SdkAgent[]
	loading: boolean
	error: string | null
	reload: () => void
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.agents(directory ?? ""),
		queryFn: async (): Promise<SdkAgent[]> => {
			const client = getProjectClient(directory!)
			if (!client) throw new Error("No client for directory")
			const result = await client.app.agents()
			const raw = (result.data ?? []) as SdkAgent[]
			return raw.filter((a) => (a.mode === "primary" || a.mode === "all") && !a.hidden)
		},
		enabled: !!directory && connected && !isMockMode,
	})

	const reload = useCallback(() => {
		if (directory) {
			queryClient.invalidateQueries({ queryKey: queryKeys.agents(directory) })
		}
	}, [directory, queryClient])

	// Return mock data if in mock mode
	if (isMockMode && directory) {
		return {
			agents: MOCK_AGENTS as unknown as SdkAgent[],
			loading: false,
			error: null,
			reload,
		}
	}

	return {
		agents: data ?? [],
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load agents") : null,
		reload,
	}
}

export function useModelState(): {
	recentModels: ModelRef[]
	loading: boolean
	error: string | null
	addRecent: (model: ModelRef) => void
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.modelState,
		queryFn: async (): Promise<ModelRef[]> => {
			const result = await fetchModelState()
			return result.recent ?? []
		},
		enabled: connected && !isMockMode,
		staleTime: 60_000,
	})

	const addRecent = useCallback(
		(model: ModelRef) => {
			queryClient.setQueryData<ModelRef[]>(queryKeys.modelState, (prev) => {
				const key = (m: ModelRef) => `${m.providerID}/${m.modelID}`
				const seen = new Set<string>()
				const updated: ModelRef[] = []
				for (const entry of [model, ...(prev ?? [])]) {
					const k = key(entry)
					if (!seen.has(k) && updated.length < 10) {
						seen.add(k)
						updated.push(entry)
					}
				}
				return updated
			})

			updateModelRecent(model).catch((err) => {
				console.error("Failed to persist model to recent:", err)
			})
		},
		[queryClient],
	)

	// Return mock data if in mock mode
	if (isMockMode) {
		return {
			recentModels: [{ providerID: "bedrock", modelID: "anthropic.claude-opus-4-6" }],
			loading: false,
			error: null,
			addRecent: () => {
				// No-op in mock mode
			},
		}
	}

	return {
		recentModels: data ?? [],
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load model state") : null,
		addRecent,
	}
}

export function useServerCommands(directory: string | null): SdkCommand[] {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)

	const { data } = useQuery({
		queryKey: queryKeys.commands(directory ?? ""),
		queryFn: async () => {
			const client = getProjectClient(directory!)
			if (!client) throw new Error("No client for directory")
			const result = await client.command.list()
			return (result.data ?? []) as SdkCommand[]
		},
		enabled: !!directory && connected && !isMockMode,
	})

	return data ?? []
}

export function useProviderVendors(): {
	data: SdkProviderVendor[] | null
	loading: boolean
	error: string | null
	reload: () => void
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.providerVendors,
		queryFn: async (): Promise<SdkProviderVendor[]> => {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			const result = await client.provider.list()
			return result.data.provider_vendors ?? []
		},
		enabled: connected && !isMockMode,
	})

	const reload = useCallback(() => {
		queryClient.invalidateQueries({ queryKey: queryKeys.providerVendors })
	}, [queryClient])

	return {
		data: data ?? null,
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load providers") : null,
		reload,
	}
}

// ============================================================
// Provider catalog types
// ============================================================

/** A provider from the full catalog (GET /provider/) */
export interface CatalogProvider {
	id: string
	name: string
	api?: string
	npm?: string
	env: string[]
	models: Record<string, unknown>
}

/** Full provider list response */
export interface AllProvidersData {
	all: CatalogProvider[]
	defaults: Record<string, string>
	connected: string[]
}

/** A connected provider with source info (from GET /config/providers) */
export interface ConnectedProviderInfo {
	id: string
	name: string
	source: "env" | "config" | "custom" | "api"
	env: string[]
}

// ============================================================
// Provider management hooks
// ============================================================

/**
 * Fetches the full provider catalog (connected and unconnected).
 * Uses GET /provider/ instead of GET /config/providers.
 */
export function useAllProviders(): {
	data: AllProvidersData | null
	loading: boolean
	error: string | null
	reload: () => void
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.allProviders,
		queryFn: async (): Promise<AllProvidersData> => {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			const result = await client.provider.list()
			const raw = result.data as {
				all: CatalogProvider[]
				default: Record<string, string>
				connected: string[]
			}
			return {
				all: raw.all ?? [],
				defaults: raw.default ?? {},
				connected: raw.connected ?? [],
			}
		},
		enabled: connected && !isMockMode,
	})

	const reload = useCallback(() => {
		queryClient.invalidateQueries({ queryKey: queryKeys.allProviders })
	}, [queryClient])

	return {
		data: data ?? null,
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load providers") : null,
		reload,
	}
}

/**
 * Fetches connected providers with their `source` field.
 * Uses GET /config/providers via the base client (no directory scope).
 * This gives us source info ("env" | "config" | "custom" | "api") that
 * the catalog endpoint (GET /provider/) does not provide.
 */
export function useConnectedProviders(): {
	data: Map<string, ConnectedProviderInfo> | null
	loading: boolean
	error: string | null
	reload: () => void
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)
	const queryClient = useQueryClient()

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.connectedProviders,
		queryFn: async (): Promise<Map<string, ConnectedProviderInfo>> => {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			const result = await client.config.providers()
			const raw = result.data as {
				providers: Array<{
					id: string
					name: string
					source: "env" | "config" | "custom" | "api"
					env: string[]
				}>
			}
			const map = new Map<string, ConnectedProviderInfo>()
			for (const p of raw.providers ?? []) {
				map.set(p.id, { id: p.id, name: p.name, source: p.source, env: p.env })
			}
			return map
		},
		enabled: connected && !isMockMode,
	})

	const reload = useCallback(() => {
		queryClient.invalidateQueries({ queryKey: queryKeys.connectedProviders })
	}, [queryClient])

	return {
		data: data ?? null,
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load providers") : null,
		reload,
	}
}

/**
 * Fetches auth methods available for each provider.
 * Uses GET /provider/auth.
 */
export function useProviderAuthMethods(): {
	data: Record<string, SdkProviderAuthMethod[]> | null
	loading: boolean
	error: string | null
} {
	const connected = useAtomValue(serverConnectedAtom)
	const isMockMode = useAtomValue(isMockModeAtom)

	const { data, isLoading, error } = useQuery({
		queryKey: queryKeys.providerAuthMethods,
		queryFn: async (): Promise<Record<string, SdkProviderAuthMethod[]>> => {
			const client = getBaseClient()
			if (!client) throw new Error("Not connected to server")
			const result = await client.provider.auth()
			return (result.data ?? {}) as Record<string, SdkProviderAuthMethod[]>
		},
		enabled: connected && !isMockMode,
	})

	return {
		data: data ?? null,
		loading: isLoading,
		error: error ? (error instanceof Error ? error.message : "Failed to load auth methods") : null,
	}
}
