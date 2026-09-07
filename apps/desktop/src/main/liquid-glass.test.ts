import { describe, expect, test } from "bun:test"
import {
	getResolvedChromeTier,
	resolveStartupWindowBackground,
	resolveTitleBarOverlay,
	resolveWindowChrome,
} from "./liquid-glass"

describe("resolveWindowChrome", () => {
	test("uses transparent acrylic chrome on Windows when opaque windows are disabled", async () => {
		const chrome = await resolveWindowChrome({
			isOpaque: false,
			isDarkMode: true,
			platform: "win32",
		})

		expect(chrome).toEqual({
			tier: "transparent",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				backgroundMaterial: "acrylic",
				resizable: true,
				maximizable: true,
				minimizable: true,
				fullscreenable: true,
				thickFrame: true,
				roundedCorners: true,
				titleBarStyle: "hidden",
				titleBarOverlay: {
					color: "#00000000",
					symbolColor: "#f4f4f5",
					height: 32,
				},
			},
		})
		expect(getResolvedChromeTier()).toBe("transparent")
	})

	test("honors opaque windows on Windows", async () => {
		const chrome = await resolveWindowChrome({
			isOpaque: true,
			isDarkMode: true,
			platform: "win32",
		})

		expect(chrome).toEqual({
			tier: "opaque",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				titleBarStyle: "hidden",
				titleBarOverlay: {
					color: "#00000000",
					symbolColor: "#f4f4f5",
					height: 32,
				},
			},
		})
		expect(getResolvedChromeTier()).toBe("opaque")
	})

	test("uses hidden titlebar overlay on Linux while keeping the opaque tier", async () => {
		const chrome = await resolveWindowChrome({
			isOpaque: false,
			isDarkMode: true,
			platform: "linux",
		})

		expect(chrome).toEqual({
			tier: "opaque",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				titleBarStyle: "hidden",
				titleBarOverlay: {
					color: "#00000000",
					symbolColor: "#f4f4f5",
					height: 32,
				},
			},
		})
		expect(getResolvedChromeTier()).toBe("opaque")
	})

	test("keeps macOS titlebar settings in opaque mode", async () => {
		const chrome = await resolveWindowChrome({ isOpaque: true, platform: "darwin" })

		expect(chrome).toEqual({
			tier: "opaque",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				titleBarStyle: "hiddenInset",
				trafficLightPosition: { x: 15, y: 15 },
			},
		})
		expect(getResolvedChromeTier()).toBe("opaque")
	})

	test("uses dark titlebar overlay symbols in light mode", () => {
		expect(resolveTitleBarOverlay(false)).toEqual({
			color: "#00000000",
			symbolColor: "#111111",
			height: 32,
		})
	})

	test("matches the native startup background to the splash theme", () => {
		expect({
			dark: resolveStartupWindowBackground(true),
			light: resolveStartupWindowBackground(false),
		}).toEqual({
			dark: "#181818",
			light: "#ffffff",
		})
	})
})
