/** Default right Changes panel width (≈40% of a typical desktop window). */
export const REVIEW_PANEL_DEFAULT_WIDTH_PX = 480
export const REVIEW_PANEL_MIN_WIDTH_PX = 280
export const REVIEW_PANEL_MAX_WIDTH_PX = 900

/** Clamp the right review panel width to usable min/max bounds. */
export function clampReviewPanelWidth(
	width: number,
	options?: { windowWidth?: number; contentMinWidth?: number },
): number {
	const windowWidth = options?.windowWidth ?? Number.POSITIVE_INFINITY
	const contentMinWidth = options?.contentMinWidth ?? 360
	const maxForWindow = Number.isFinite(windowWidth)
		? Math.max(REVIEW_PANEL_MIN_WIDTH_PX, windowWidth - contentMinWidth)
		: REVIEW_PANEL_MAX_WIDTH_PX
	const max = Math.min(REVIEW_PANEL_MAX_WIDTH_PX, maxForWindow)
	if (!Number.isFinite(width)) return REVIEW_PANEL_DEFAULT_WIDTH_PX
	return Math.min(max, Math.max(REVIEW_PANEL_MIN_WIDTH_PX, Math.round(width)))
}
