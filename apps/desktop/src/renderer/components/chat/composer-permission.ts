/**
 * Session permission profiles for the composer footer picker.
 *
 * Labels match the TUI permission preset picker so Desktop and terminal
 * describe the same three modes.
 */

export type ComposerPermissionProfile = "default" | "autoReview" | "fullAccess"

/** Matches server `new_session_state`: new sessions default to AutoReview. */
export const DEFAULT_COMPOSER_PERMISSION_PROFILE: ComposerPermissionProfile = "autoReview"

export const COMPOSER_PERMISSION_PROFILES: readonly {
	id: ComposerPermissionProfile
	label: string
	description: string
}[] = [
	{
		id: "default",
		label: "Ask for approval",
		description: "You approve sensitive tools",
	},
	{
		id: "autoReview",
		label: "Approve for me",
		description: "An AI reviewer may approve low-risk tools",
	},
	{
		id: "fullAccess",
		label: "Full access",
		description: "No sandbox and no approval prompts",
	},
]

export function parseComposerPermissionProfile(
	value: string | undefined | null,
): ComposerPermissionProfile {
	switch (value) {
		case "default":
			return "default"
		case "autoReview":
		case "auto-review":
			return "autoReview"
		case "fullAccess":
		case "full-access":
			return "fullAccess"
		default:
			return DEFAULT_COMPOSER_PERMISSION_PROFILE
	}
}

export function composerPermissionLabel(profile: ComposerPermissionProfile): string {
	return (
		COMPOSER_PERMISSION_PROFILES.find((entry) => entry.id === profile)?.label ?? "Ask for approval"
	)
}
