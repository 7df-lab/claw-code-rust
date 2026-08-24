import { describe, expect, mock, test } from "bun:test"
import type React from "react"
import { renderToStaticMarkup } from "react-dom/server"

const storage = new Map<string, string>()

Object.defineProperty(globalThis, "localStorage", {
	value: {
		getItem: (key: string) => storage.get(key) ?? null,
		removeItem: (key: string) => {
			storage.delete(key)
		},
		setItem: (key: string, value: string) => {
			storage.set(key, value)
		},
	},
})

mock.module("@devo/ui/components/select", () => ({
	Select: ({ children }: { children: React.ReactNode }) => <div data-slot="select">{children}</div>,
	SelectContent: ({ children }: { children: React.ReactNode }) => (
		<div data-slot="select-content">{children}</div>
	),
	SelectGroup: ({ children }: { children: React.ReactNode }) => (
		<div data-slot="select-group">{children}</div>
	),
	SelectItem: ({ children, value }: { children: React.ReactNode; value: string }) => (
		<div data-slot="select-item" data-value={value}>
			{children}
		</div>
	),
	SelectLabel: ({ children }: { children: React.ReactNode }) => (
		<div data-slot="select-label">{children}</div>
	),
	SelectScrollDownButton: () => null,
	SelectScrollUpButton: () => null,
	SelectSeparator: () => <hr />,
	SelectTrigger: ({ children }: { children: React.ReactNode }) => (
		<button type="button" data-slot="select-trigger">
			{children}
		</button>
	),
	SelectValue: () => <span data-slot="select-value" />,
}))

mock.module("@devo/ui/components/switch", () => ({
	Switch: ({ checked }: { checked: boolean }) => (
		<button type="button" aria-pressed={checked} data-slot="switch" />
	),
}))

const { GeneralSettings } = await import("./general-settings")

describe("GeneralSettings", () => {
	test("keeps appearance controls visible", () => {
		const markup = renderToStaticMarkup(<GeneralSettings />)

		expect({
			hasAppearance: markup.includes(">Appearance</h3>"),
			hasTheme: markup.includes(">Theme</label>"),
			hasDarkMode: markup.includes(">Dark</button>") || markup.includes(">Dark</"),
			hasDisplayMode: markup.includes(">Display mode</label>"),
			hasVerboseMode: markup.includes(">Verbose</div>") || markup.includes(">Verbose<"),
			hasConversation: markup.includes(">Conversation</h3>"),
			hasHideThinking: markup.includes(">Hide thinking while working</label>"),
			hasHomepageTitle: markup.includes("text-[32px] font-normal leading-tight tracking-[-0.03em]"),
		}).toEqual({
			hasAppearance: true,
			hasTheme: true,
			hasDarkMode: true,
			hasDisplayMode: true,
			hasVerboseMode: true,
			hasConversation: true,
			hasHideThinking: true,
			hasHomepageTitle: true,
		})
	})
})
