import { atom } from "jotai"
import { atomFamily } from "jotai-family"

export interface SessionNativeState {
	commands: unknown[]
	configOptions: unknown[]
	modeID?: string
	usage?: {
		used: unknown
		size: unknown
		cost?: unknown
	}
}

export const sessionNativeFamily = atomFamily((_sessionId: string) =>
	atom<SessionNativeState>({
		commands: [],
		configOptions: [],
	}),
)
