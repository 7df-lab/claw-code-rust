/** Default expanded sidebar width in px (matches UI `SIDEBAR_WIDTH` 17.5rem at 16px root). */
export const SIDEBAR_DEFAULT_WIDTH_PX = 280
export const SIDEBAR_MIN_WIDTH_PX = 200
export const SIDEBAR_MAX_WIDTH_PX = 480

/** Clamp a sidebar width to usable min/max bounds. */
export function clampSidebarWidth(
	width: number,
	options?: { windowWidth?: number; contentMinWidth?: number },
): number {
	const windowWidth = options?.windowWidth ?? Number.POSITIVE_INFINITY
	const contentMinWidth = options?.contentMinWidth ?? 360
	const maxForWindow = Number.isFinite(windowWidth)
		? Math.max(SIDEBAR_MIN_WIDTH_PX, windowWidth - contentMinWidth)
		: SIDEBAR_MAX_WIDTH_PX
	const max = Math.min(SIDEBAR_MAX_WIDTH_PX, maxForWindow)
	if (!Number.isFinite(width)) return SIDEBAR_DEFAULT_WIDTH_PX
	return Math.min(max, Math.max(SIDEBAR_MIN_WIDTH_PX, Math.round(width)))
}
