import type { PermissionRuleset } from "@devo-ai/sdk/v2/client"
import type { PermissionPreset } from "./types"

/**
 * Builds the explicit unattended policy for one automation preset.
 * The default deliberately installs no rules, so an unconfigured automation
 * cannot silently approve or reject an interactive request.
 */
export function buildPermissionRuleset(preset: PermissionPreset): PermissionRuleset {
	const nonInteractiveRules: PermissionRuleset = [
		{ permission: "question", pattern: "*", action: "deny" },
		{ permission: "plan_enter", pattern: "*", action: "deny" },
		{ permission: "plan_exit", pattern: "*", action: "deny" },
	]

	switch (preset) {
		case "allow-all":
			return [
				{ permission: "*", pattern: "*", action: "allow" },
				...nonInteractiveRules,
				{ permission: "edit", pattern: "*", action: "allow" },
				{ permission: "bash", pattern: "*", action: "allow" },
				{ permission: "webfetch", pattern: "*", action: "allow" },
				{ permission: "external_directory", pattern: "*", action: "allow" },
			]
		case "read-only":
			return [
				...nonInteractiveRules,
				{ permission: "edit", pattern: "*", action: "deny" },
				{ permission: "bash", pattern: "*", action: "deny" },
				{ permission: "webfetch", pattern: "*", action: "allow" },
			]
		case "default":
			return []
	}
}
