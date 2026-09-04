/** Canonical Native provider/upsert payload shared by external config importers. */
export interface CanonicalProviderUpsertParams {
	provider: {
		id: string
		name: string
		baseUrl?: string
		wireApis: string[]
		models: Record<string, { name?: string }>
		enabled: true
	}
	defaultModel?: string
	smallModel?: string
	apiKey?: string
}
