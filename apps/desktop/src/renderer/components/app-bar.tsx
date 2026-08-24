import { useAppBarContent } from "./app-bar-context"

// Height of the app bar in pixels — used as CSS variable
export const APP_BAR_HEIGHT = 40

/**
 * Detect whether we're running inside Electron (preload injects `window.devo`).
 */
function isElectron(): boolean {
	return typeof window !== "undefined" && "devo" in window
}

export function AppBar() {
	const pageContent = useAppBarContent()

	return (
		<div
			data-slot="app-bar"
			className="relative z-30 flex shrink-0 items-center border-b border-border/30 pl-4 pr-[var(--window-controls-right-inset)] transition-[background-color,padding-left,padding-right] duration-250 ease-in-out group-data-[state=collapsed]/sidebar-wrapper:pl-[var(--window-controls-inset)]"
			style={{
				height: APP_BAR_HEIGHT,
				// Make entire bar draggable on Electron (title bar replacement)
				// @ts-expect-error -- vendor-prefixed CSS property
				WebkitAppRegion: isElectron() ? "drag" : undefined,
			}}
		>
			{/* ===== Page content (via portal) ===== */}
			<div className="relative flex h-full min-w-0 flex-1 items-center">{pageContent}</div>
		</div>
	)
}
