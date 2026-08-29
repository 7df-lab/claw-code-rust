import { beforeEach, describe, expect, mock, test } from "bun:test"

const setOption = mock(async () => undefined)
const invalidateQueries = mock(async () => undefined)

mock.module("../services/connection-manager", () => ({
	getProjectClient: () => ({
		config: { setOption },
	}),
}))

mock.module("./query-client", () => ({
	queryClient: { invalidateQueries },
}))

const { persistRuntimeModelConfigOption, persistRuntimeModelSelection } = await import(
	"./model-config-options"
)

describe("runtime model config option persistence", () => {
	beforeEach(() => {
		setOption.mockClear()
		invalidateQueries.mockClear()
	})

	test("persists selected model through runtime config", async () => {
		await persistRuntimeModelSelection("/repo", {
			providerID: "session",
			modelID: "deepseek-v4-flash",
		})

		expect(setOption).toHaveBeenCalledWith({
			configID: "model",
			value: "deepseek-v4-flash",
		})
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["providers", "/repo"] })
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["config", "/repo"] })
	})

	test("persists selected reasoning effort through runtime config", async () => {
		await persistRuntimeModelConfigOption("/repo", "thought_level", "max")

		expect(setOption).toHaveBeenCalledWith({
			configID: "thought_level",
			value: "max",
		})
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["providers", "/repo"] })
		expect(invalidateQueries).toHaveBeenCalledWith({ queryKey: ["config", "/repo"] })
	})
})
