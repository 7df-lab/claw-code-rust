import { atom } from "jotai"
import type { DevoProject } from "../lib/types"

// ============================================================
// Types
// ============================================================

/**
 * Granular startup phase for UI feedback.
 *
 * - `idle`: initial state, discovery hasn't started
 * - `starting-server`: waiting for the Devo server to start/connect
 * - `connecting`: establishing Native event stream
 * - `loading-projects`: fetching the project list from the API
 * - `loading-sessions`: fetching sessions for discovered projects
 * - `ready`: discovery complete, app is usable
 * - `error`: discovery failed
 */
export type DiscoveryPhase =
	| "idle"
	| "starting-server"
	| "connecting"
	| "loading-projects"
	| "loading-sessions"
	| "ready"
	| "error"

/**
 * State for discovered project/session data.
 *
 * With API-first discovery, projects come from `client.project.list()` and
 * sessions are loaded per-project via `client.session.list()`. The discovery
 * atom only tracks projects now; sessions are loaded directly into the
 * session atom family by the connection manager.
 */
export interface DiscoveryState {
	loaded: boolean
	loading: boolean
	error: string | null
	/** Granular startup phase for loading UI */
	phase: DiscoveryPhase
	/** Projects discovered from the Devo API (Project type from SDK) */
	projects: DevoProject[]
}

// ============================================================
// Atoms
// ============================================================

export const discoveryAtom = atom<DiscoveryState>({
	loaded: false,
	loading: false,
	error: null,
	phase: "idle",
	projects: [],
})

// Convenience selectors
export const discoveryLoadedAtom = atom((get) => get(discoveryAtom).loaded)
export const discoveryLoadingAtom = atom((get) => get(discoveryAtom).loading)
export const discoveryPhaseAtom = atom((get) => get(discoveryAtom).phase)
export const discoveryProjectsAtom = atom((get) => get(discoveryAtom).projects)
