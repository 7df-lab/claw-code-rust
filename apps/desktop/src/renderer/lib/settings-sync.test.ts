import { describe, expect, test } from "bun:test"
import {
	buildAppearanceSettingsPatch,
	buildRendererPreferencesMigrationPatch,
} from "./settings-sync"
import type { AppSettings } from "../../preload/api"

class MemoryStorage implements Storage {
	private readonly values = new Map<string, string>()

	get length(): number {
		return this.values.size
	}

	clear(): void {
		this.values.clear()
	}

	getItem(key: string): string | null {
		return this.values.get(key) ?? null
	}

	key(index: number): string | null {
		return Array.from(this.values.keys())[index] ?? null
	}

	removeItem(key: string): void {
		this.values.delete(key)
	}

	setItem(key: string, value: string): void {
		this.values.set(key, value)
	}
}

const defaultSettings: AppSettings = {
	notifications: {
		completionMode: "unfocused",
		permissions: true,
		questions: true,
		errors: true,
		dockBadge: true,
	},
	opaqueWindows: false,
	appearance: {
		colorScheme: "light",
		themeId: "default",
		displayMode: "default",
		hideThinkingWhileWorking: true,
		rendererPreferencesMigrated: false,
	},
	openIn: {
		preferredTargetId: null,
	},
	desktopFolders: {
		folders: [],
	},
	servers: {
		servers: [{ id: "local", name: "This Mac", type: "local" }],
		activeServerId: "local",
		networkProxy: {
			mode: "system",
			proxyUrl: "",
			noProxy: "localhost,127.0.0.1,::1",
		},
	},
}

describe("renderer settings sync", () => {
	test("builds a one-time migration patch from existing renderer localStorage", () => {
		const storage = new MemoryStorage()
		storage.setItem("devo:colorScheme", JSON.stringify("system"))
		storage.setItem("devo:theme", JSON.stringify("cortex"))
		storage.setItem("devo:displayMode", JSON.stringify("verbose"))

		const patch = buildRendererPreferencesMigrationPatch(defaultSettings, storage)

		expect(patch).toEqual({
			appearance: {
				colorScheme: "system",
				themeId: "cortex",
				displayMode: "verbose",
				rendererPreferencesMigrated: true,
			},
		})
	})

	test("marks explicit color-scheme writes as renderer-preferences migrated", () => {
		expect(buildAppearanceSettingsPatch({ colorScheme: "light" })).toEqual({
			appearance: {
				colorScheme: "light",
				rendererPreferencesMigrated: true,
			},
		})
	})
})
