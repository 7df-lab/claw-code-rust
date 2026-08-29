import { queryKeys, type ModelRef } from "../hooks/use-devo-data"
import { getProjectClient } from "../services/connection-manager"
import { queryClient } from "./query-client"

export type RuntimeModelConfigID = "model" | "thought_level"

export async function persistRuntimeModelConfigOption(
	directory: string,
	configID: RuntimeModelConfigID,
	value: string,
): Promise<void> {
	const client = getProjectClient(directory)
	if (!client) throw new Error(`No client for directory ${directory}`)
	await client.config.setOption({ configID, value })
	await Promise.all([
		queryClient.invalidateQueries({ queryKey: queryKeys.providers(directory) }),
		queryClient.invalidateQueries({ queryKey: queryKeys.config(directory) }),
	])
}

export async function persistRuntimeModelSelection(
	directory: string,
	model: ModelRef,
): Promise<void> {
	await persistRuntimeModelConfigOption(directory, "model", model.modelID)
}
