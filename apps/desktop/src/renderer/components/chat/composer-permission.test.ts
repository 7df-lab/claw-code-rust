import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import {
	COMPOSER_PERMISSION_PROFILES,
	composerPermissionLabel,
	parseComposerPermissionProfile,
} from "./composer-permission"

const pickerSource = readFileSync(new URL("./composer-permission-picker.tsx", import.meta.url), "utf8")
const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
const newChatSource = readFileSync(new URL("../new-chat.tsx", import.meta.url), "utf8")

describe("composer permission profiles", () => {
	test("uses the TUI labels for the three presets", () => {
		expect(COMPOSER_PERMISSION_PROFILES.map((profile) => profile.label)).toEqual([
			"Ask for approval",
			"Approve for me",
			"Full access",
		])
		expect(parseComposerPermissionProfile("auto-review")).toBe("autoReview")
		expect(parseComposerPermissionProfile("fullAccess")).toBe("fullAccess")
		expect(parseComposerPermissionProfile("default")).toBe("default")
		expect(parseComposerPermissionProfile(undefined)).toBe("autoReview")
		expect(composerPermissionLabel("autoReview")).toBe("Approve for me")
		expect(composerPermissionLabel("default")).toBe("Ask for approval")
	})

	test("places a compact footer picker on both composers without a search field", () => {
		expect({
			usesOptionMenu: pickerSource.includes("optionMenuContentClass"),
			sidebarIconStroke: pickerSource.includes("stroke-[1.5]"),
			noSearchField: !pickerSource.includes("placeholder") && !pickerSource.includes("SearchIcon"),
			chatViewWiresPicker: chatViewSource.includes("ComposerPermissionPicker"),
			newChatWiresPicker: newChatSource.includes("ComposerPermissionPicker"),
			defaultsToAutoReview:
				chatViewSource.includes("DEFAULT_COMPOSER_PERMISSION_PROFILE") &&
				newChatSource.includes("DEFAULT_COMPOSER_PERMISSION_PROFILE"),
			permissionSitsWithAttach:
				chatViewSource.includes("<AttachButton") &&
				chatViewSource.indexOf("<AttachButton") < chatViewSource.indexOf("<ComposerPermissionPicker"),
			modelSitsWithSend:
				chatViewSource.includes('className="ml-auto flex min-w-0 items-center gap-0.5"') &&
				newChatSource.includes('className="ml-auto flex min-w-0 items-center gap-0.5"'),
		}).toEqual({
			usesOptionMenu: true,
			sidebarIconStroke: true,
			noSearchField: true,
			chatViewWiresPicker: true,
			newChatWiresPicker: true,
			defaultsToAutoReview: true,
			permissionSitsWithAttach: true,
			modelSitsWithSend: true,
		})
	})
})
