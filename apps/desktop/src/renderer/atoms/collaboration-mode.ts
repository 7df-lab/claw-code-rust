import { atom } from "jotai"
import { atomFamily } from "jotai-family"

export type CollaborationMode = "build" | "plan"

export const collaborationModeFamily = atomFamily((_sessionId: string) =>
	atom<CollaborationMode>("build"),
)
