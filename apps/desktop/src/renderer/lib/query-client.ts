import { QueryClient } from "@tanstack/react-query"

/**
 * Single QueryClient instance shared between the React tree (via QueryClientProvider)
 * and imperative code (e.g. the Native event processor that needs to invalidate queries).
 */
export const queryClient = new QueryClient({
	defaultOptions: {
		queries: {
			// Data like providers, config, agents rarely changes — keep it fresh for 5 minutes.
			// This eliminates redundant fetches when navigating between sessions in the same project.
			staleTime: 5 * 60 * 1000,
			// Keep unused query data in cache for 10 minutes (default is 5).
			gcTime: 10 * 60 * 1000,
			// Don't refetch on window focus by default — Native events handle real-time updates.
			refetchOnWindowFocus: false,
			// Single retry with 1s delay for transient network errors.
			retry: 1,
			retryDelay: 1000,
		},
	},
})
