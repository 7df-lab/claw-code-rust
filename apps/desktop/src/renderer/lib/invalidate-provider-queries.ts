import { invalidateConfigOptionCaches } from "../services/connection-manager"
import { queryClient } from "./query-client"

/**
 * After provider/model catalog mutations (connect, discover, enable, upsert),
 * refresh both the Settings catalog and the composer ModelSelector data path
 * (`config.providers` → React Query `providers` / `config`).
 *
 * Query key prefixes match `queryKeys` in use-devo-data.ts.
 */
export function invalidateProviderDependentQueries(): void {
	invalidateConfigOptionCaches()
	void queryClient.invalidateQueries({ queryKey: ["providerCatalog"] })
	void queryClient.invalidateQueries({ queryKey: ["allProviders"] })
	void queryClient.invalidateQueries({ queryKey: ["connectedProviders"] })
	// Prefix match: ["providers", directory] and ["config", directory]
	void queryClient.invalidateQueries({ queryKey: ["providers"] })
	void queryClient.invalidateQueries({ queryKey: ["config"] })
}
