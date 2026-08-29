import { atom } from "jotai"
import { atomWithStorage } from "jotai/utils"
import type { DisplayMode, WindowChromeTier } from "../../preload/api"
import { DEFAULT_APPEARANCE_SETTINGS } from "../../shared/app-settings"
import type { ColorScheme } from "../lib/themes"
import { SIDEBAR_DEFAULT_WIDTH_PX } from "../lib/sidebar-width"

export {
	clampSidebarWidth,
	SIDEBAR_DEFAULT_WIDTH_PX,
	SIDEBAR_MAX_WIDTH_PX,
	SIDEBAR_MIN_WIDTH_PX,
} from "../lib/sidebar-width"

// ============================================================
// Types
// ============================================================

export type { DisplayMode }

export interface PersistedModelRef {
	providerID: string
	modelID: string
	variant?: string
	agent?: string
}

// ============================================================
// One-time migration from Zustand persist to Jotai atomWithStorage
// ============================================================

function migrateFromZustandPersist(): void {
	const oldKey = "devo-preferences"
	const raw = localStorage.getItem(oldKey)
	if (!raw) return

	try {
		const { state } = JSON.parse(raw) // Zustand persist wraps in { state, version }
		if (state.displayMode)
			localStorage.setItem("devo:displayMode", JSON.stringify(state.displayMode))
		if (state.theme) localStorage.setItem("devo:theme", JSON.stringify(state.theme))
		if (state.colorScheme)
			localStorage.setItem("devo:colorScheme", JSON.stringify(state.colorScheme))
		if (state.drafts) localStorage.setItem("devo:drafts", JSON.stringify(state.drafts))
		if (state.projectModels)
			localStorage.setItem("devo:projectModels", JSON.stringify(state.projectModels))

		// Remove old key after successful migration
		localStorage.removeItem(oldKey)
	} catch {
		// Ignore malformed data
	}
}

// Run migration at module load time (before any atoms are read)
migrateFromZustandPersist()

// Migrate removed "compact" display mode to "default"
function migrateDisplayMode(): void {
	const raw = localStorage.getItem("devo:displayMode")
	if (raw === '"compact"') {
		localStorage.setItem("devo:displayMode", '"default"')
	}
}
migrateDisplayMode()

// ============================================================
// Persisted atoms — each is independent with its own localStorage key
// ============================================================

export const displayModeAtom = atomWithStorage<DisplayMode>(
	"devo:displayMode",
	DEFAULT_APPEARANCE_SETTINGS.displayMode,
)

export const hideThinkingWhileWorkingAtom = atomWithStorage<boolean>(
	"devo:hideThinkingWhileWorking",
	DEFAULT_APPEARANCE_SETTINGS.hideThinkingWhileWorking,
)

export const themeAtom = atomWithStorage<string>("devo:theme", DEFAULT_APPEARANCE_SETTINGS.themeId)

export const colorSchemeAtom = atomWithStorage<ColorScheme>(
	"devo:colorScheme",
	DEFAULT_APPEARANCE_SETTINGS.colorScheme,
)

/**
 * Whether the user prefers opaque (non-transparent) windows.
 * When true, the renderer uses solid backgrounds instead of semi-transparent.
 */
export const opaqueWindowsAtom = atomWithStorage<boolean>("devo:opaqueWindows", false)

/**
 * The active window chrome tier, set by the main process on load.
 * "liquid-glass" = macOS 26+, "vibrancy" = older macOS, "transparent" = Windows,
 * "opaque" = Linux or user pref.
 * Defaults to "opaque" for browser-mode dev (no Electron).
 */
export const chromeTierAtom = atom<WindowChromeTier>("opaque")

/**
 * Whether the window has any form of transparency (liquid glass or vibrancy).
 * Used by CSS to decide between semi-transparent and solid backgrounds.
 */
export const isTransparentAtom = atom((get) => {
	const tier = get(chromeTierAtom)
	const opaque = get(opaqueWindowsAtom)
	return !opaque && (tier === "liquid-glass" || tier === "vibrancy" || tier === "transparent")
})

export const draftsAtom = atomWithStorage<Record<string, string>>("devo:drafts", {})

export const projectModelsAtom = atomWithStorage<Record<string, PersistedModelRef>>(
	"devo:projectModels",
	{},
)

/** Last project directory used for New chat / composer context. */
export const lastProjectDirectoryAtom = atomWithStorage<string | null>(
	"devo:last-project-directory",
	null,
)

/**
 * User-resized app sidebar width in pixels.
 * Applied as `--sidebar-width` on the sidebar wrapper.
 */
export const sidebarWidthAtom = atomWithStorage<number>(
	"devo:sidebarWidth",
	SIDEBAR_DEFAULT_WIDTH_PX,
)

/**
 * Whether the user has dismissed the automations permissions info banner.
 * Once dismissed, the banner never reappears.
 */
export const automationsBannerDismissedAtom = atomWithStorage<boolean>(
	"devo:automationsBannerDismissed",
	false,
)

// ============================================================
// Derived atoms for drafts
// ============================================================

/** Read a draft for a specific key */
export const readDraftAtom = (key: string) => atom((get) => get(draftsAtom)[key] ?? "")

/** Set a draft for a specific key (write-only action atom) */
export const setDraftAtom = atom(null, (get, set, args: { key: string; text: string }) => {
	const drafts = { ...get(draftsAtom) }
	if (args.text) {
		drafts[args.key] = args.text
	} else {
		delete drafts[args.key]
	}
	set(draftsAtom, drafts)
})

/** Clear a draft (write-only action atom) */
export const clearDraftAtom = atom(null, (get, set, key: string) => {
	const drafts = { ...get(draftsAtom) }
	delete drafts[key]
	set(draftsAtom, drafts)
})

/** Set a project model (write-only action atom) */
export const setProjectModelAtom = atom(
	null,
	(
		get,
		set,
		args: {
			directory: string
			model: PersistedModelRef
		},
	) => {
		const models = { ...get(projectModelsAtom) }
		models[args.directory] = {
			providerID: args.model.providerID,
			modelID: args.model.modelID,
			variant: args.model.variant,
			agent: args.model.agent,
		}
		set(projectModelsAtom, models)
	},
)
