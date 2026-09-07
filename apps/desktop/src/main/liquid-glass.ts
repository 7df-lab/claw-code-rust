/**
 * Liquid Glass — Three-tier window chrome system
 *
 * Implements progressive transparency for macOS:
 *   Tier 1: Liquid Glass (macOS 26+ Tahoe) — native NSGlassEffectView
 *   Tier 2: Vibrancy fallback (older macOS) — NSVisualEffectView via Electron
 *   Windows: Transparent acrylic — resizable BrowserWindow with native backdrop
 *   Tier 3: Opaque (user preference or Linux) — solid background
 *
 */

import type { BrowserWindow, BrowserWindowConstructorOptions, TitleBarOverlay } from "electron"
import { createLogger } from "./logger"

const log = createLogger("liquid-glass")

// ============================================================
// Types
// ============================================================

export type WindowChromeTier = "liquid-glass" | "vibrancy" | "transparent" | "opaque"

export interface WindowChromeResult {
	tier: WindowChromeTier
	usesTransparentWindow: boolean
	usesTransparentBackground: boolean
	options: Partial<BrowserWindowConstructorOptions>
}

export interface ResolveWindowChromeOptions {
	isOpaque: boolean
	isDarkMode?: boolean
	platform?: NodeJS.Platform
}

const TITLE_BAR_OVERLAY_HEIGHT = 32
const TITLE_BAR_OVERLAY_COLOR = "#00000000"
const TITLE_BAR_OVERLAY_DARK_SYMBOL_COLOR = "#111111"
const TITLE_BAR_OVERLAY_LIGHT_SYMBOL_COLOR = "#f4f4f5"
const STARTUP_WINDOW_DARK_BACKGROUND = "#181818"
const STARTUP_WINDOW_LIGHT_BACKGROUND = "#ffffff"

export function resolveTitleBarOverlay(isDarkMode: boolean): TitleBarOverlay {
	return {
		color: TITLE_BAR_OVERLAY_COLOR,
		symbolColor: isDarkMode
			? TITLE_BAR_OVERLAY_LIGHT_SYMBOL_COLOR
			: TITLE_BAR_OVERLAY_DARK_SYMBOL_COLOR,
		height: TITLE_BAR_OVERLAY_HEIGHT,
	}
}

export function resolveStartupWindowBackground(isDarkMode: boolean): string {
	return isDarkMode ? STARTUP_WINDOW_DARK_BACKGROUND : STARTUP_WINDOW_LIGHT_BACKGROUND
}

// ============================================================
// Liquid glass support detection (cached singleton)
// ============================================================

let _glassSupport: boolean | null = null
// biome-ignore lint: dynamic import type for optional macOS-only native module
let _glassModule: any = null
let _resolvedTier: WindowChromeTier = "opaque"

/**
 * Get the last resolved chrome tier.
 * Available after resolveWindowChrome() has been called.
 */
export function getResolvedChromeTier(): WindowChromeTier {
	return _resolvedTier
}

/**
 * Check if liquid glass is supported on this platform.
 * Result is cached after first call.
 */
export async function isLiquidGlassSupported(): Promise<boolean> {
	if (_glassSupport !== null) return _glassSupport

	try {
		// Dynamic import — electron-liquid-glass is a macOS-only optional native module
		// and may not be present on other platforms or in CI environments.
		// Use a variable to prevent static module resolution in tsgo on Linux CI.
		const moduleName = "electron-liquid-glass"
		const mod = await import(/* @vite-ignore */ moduleName)
		_glassModule = mod
		const glass = mod.default
		_glassSupport = glass.isGlassSupported() as boolean
		log.info(`Liquid glass supported: ${_glassSupport}`)
	} catch (err) {
		log.warn("Failed to load electron-liquid-glass:", err)
		_glassSupport = false
	}

	return _glassSupport as boolean
}

/**
 * Get the cached liquid glass module, or null if not available.
 */
function getGlassModule() {
	return _glassModule
}

// ============================================================
// Window chrome resolution
// ============================================================

/**
 * Resolves the window chrome configuration based on platform capabilities
 * and user preferences.
 *
 * @param options.isOpaque - Whether the user has opted for opaque windows
 * @param options.platform - Host platform, injectable for tests
 * @returns BrowserWindow options to spread into the constructor
 */
export async function resolveWindowChrome({
	isOpaque,
	isDarkMode = false,
	platform = process.platform,
}: ResolveWindowChromeOptions): Promise<WindowChromeResult> {
	const isMac = platform === "darwin"
	const isWindows = platform === "win32"
	const isLinux = platform === "linux"

	if (isWindows) {
		if (isOpaque) {
			log.info("Using opaque window chrome on Windows")
			_resolvedTier = "opaque"
			return {
				tier: "opaque",
				usesTransparentWindow: false,
				usesTransparentBackground: false,
				options: {
					titleBarStyle: "hidden" as const,
					titleBarOverlay: resolveTitleBarOverlay(isDarkMode),
				},
			}
		}

		log.info("Using transparent acrylic window chrome on Windows")
		_resolvedTier = "transparent"
		return {
			tier: "transparent",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				backgroundMaterial: "acrylic" as const,
				resizable: true,
				maximizable: true,
				minimizable: true,
				fullscreenable: true,
				thickFrame: true,
				roundedCorners: true,
				titleBarStyle: "hidden" as const,
				titleBarOverlay: resolveTitleBarOverlay(isDarkMode),
			},
		}
	}

	// Tier 3: Opaque — user preference or Linux
	if (isOpaque || !isMac) {
		log.info("Using opaque window chrome (tier 3)")
		_resolvedTier = "opaque"
		return {
			tier: "opaque",
			usesTransparentWindow: false,
			usesTransparentBackground: false,
			options: {
				...(isLinux && {
					titleBarStyle: "hidden" as const,
					titleBarOverlay: resolveTitleBarOverlay(isDarkMode),
				}),
				...(isMac && {
					titleBarStyle: "hiddenInset" as const,
					trafficLightPosition: { x: 15, y: 15 },
				}),
			},
		}
	}

	// Check liquid glass support
	const glassSupported = await isLiquidGlassSupported()

	// Tier 1: Liquid Glass — macOS 26+ (Tahoe)
	if (glassSupported) {
		log.info("Using liquid glass window chrome (tier 1)")
		_resolvedTier = "liquid-glass"
		return {
			tier: "liquid-glass",
			usesTransparentWindow: true,
			usesTransparentBackground: true,
			options: {
				transparent: true,
				titleBarStyle: "hiddenInset" as const,
				trafficLightPosition: { x: 15, y: 15 },
			},
		}
	}

	// Tier 2: Vibrancy — older macOS
	log.info("Using vibrancy window chrome (tier 2)")
	_resolvedTier = "vibrancy"
	return {
		tier: "vibrancy",
		usesTransparentWindow: false,
		usesTransparentBackground: true,
		options: {
			vibrancy: "menu" as const,
			visualEffectState: "active" as const,
			titleBarStyle: "hiddenInset" as const,
			trafficLightPosition: { x: 15, y: 15 },
		},
	}
}

// ============================================================
// Post-creation glass installation
// ============================================================

/**
 * Install liquid glass effect on a BrowserWindow after creation.
 * Must be called after the window is created and ideally after
 * the page has finished loading.
 *
 * If liquid glass fails, falls back to vibrancy.
 *
 * @param win - The BrowserWindow to apply glass to
 * @param isOpaque - Whether to use opaque mode (passes opaque flag to native)
 */
export async function installLiquidGlass(win: BrowserWindow, isOpaque: boolean): Promise<void> {
	const mod = getGlassModule()
	if (!mod) {
		log.warn("Cannot install liquid glass — module not loaded")
		return
	}

	const glass = mod.default

	// Ensure the page has loaded before applying glass
	const applyGlass = () => {
		try {
			win.setWindowButtonVisibility(true)

			const handle = win.getNativeWindowHandle()
			const viewId = glass.addView(handle, isOpaque ? { opaque: true } : {})

			if (viewId === -1) {
				// Glass failed — fall back to vibrancy
				log.warn("Liquid glass addView returned -1, falling back to vibrancy")
				win.setVibrancy("menu")
				return
			}

			log.info(`Liquid glass installed (viewId: ${viewId}, opaque: ${isOpaque})`)
		} catch (err) {
			log.error("Failed to install liquid glass:", err)
			// Fall back to vibrancy on error
			try {
				win.setVibrancy("menu")
			} catch {
				// Ignore vibrancy fallback errors
			}
		}
	}

	// Apply glass once the page finishes loading
	if (win.webContents.isLoading()) {
		win.webContents.once("did-finish-load", applyGlass)
	} else {
		applyGlass()
	}
}
