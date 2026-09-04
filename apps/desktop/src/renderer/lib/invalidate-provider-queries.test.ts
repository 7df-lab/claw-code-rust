import { beforeEach, describe, expect, mock, test } from "bun:test"

const invalidateConfigOptionCaches = mock(() => undefined)
const invalidateQueries = mock(async () => undefined)

mock.module("../services/connection-manager", () => ({
	invalidateConfigOptionCaches,
}))

mock.module("./query-client", () => ({
	queryClient: { invalidateQueries },
}))

const { invalidateProviderDependentQueries } = await import("./invalidate-provider-queries")

describe("invalidateProviderDependentQueries", () => {
	beforeEach(() => {
		invalidateConfigOptionCaches.mockClear()
		invalidateQueries.mockClear()
	})

	test("clears SDK config caches and composer provider queries", () => {
		invalidateProviderDependentQueries()

		expect(invalidateConfigOptionCaches).toHaveBeenCalledTimes(1)
		const keys = invalidateQueries.mock.calls.map((call) => call[0]?.queryKey)
		expect(keys).toContainEqual(["providerCatalog"])
		expect(keys).toContainEqual(["allProviders"])
		expect(keys).toContainEqual(["connectedProviders"])
		expect(keys).toContainEqual(["providers"])
		expect(keys).toContainEqual(["config"])
	})
})
