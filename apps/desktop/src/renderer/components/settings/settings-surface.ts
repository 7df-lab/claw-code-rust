/**
 * Shared surface recipes for Settings and Customize.
 *
 * Flat, token-aligned cards — no soft drop shadows, no oversized radii.
 * Matches dialogs (`rounded-xl ring-1`) and sidebar density.
 */

/** Primary list/card surface used by SettingsSection and empty states. */
export const settingsCardClass =
	"overflow-hidden rounded-xl border border-border/50 bg-card divide-y divide-border/40"

/** Card without row dividers (empty states, dashed placeholders). */
export const settingsPanelClass =
	"overflow-hidden rounded-xl border border-border/50 bg-card"

/** Soft error / status banner. */
export const settingsBannerClass =
	"flex items-center gap-3 rounded-xl border border-destructive/30 bg-destructive/5 px-4 py-3 text-sm text-destructive"

/** Dashed empty placeholder. */
export const settingsDashedClass =
	"rounded-xl border border-dashed border-border/50 py-10 text-center"

/** Vertical page rhythm for settings routes. */
export const settingsPageClass = "flex flex-col gap-8"
