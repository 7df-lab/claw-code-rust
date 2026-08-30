export const SCROLL_BOTTOM_THRESHOLD_PX = 48
export const SCROLL_RESTORE_READY_TIMEOUT_MS = 2_000

export interface SessionScrollSnapshot {
	scrollTop: number
	atBottom: boolean
	hasSnapshot: boolean
}

export const EMPTY_SESSION_SCROLL_SNAPSHOT: SessionScrollSnapshot = {
	scrollTop: 0,
	atBottom: true,
	hasSnapshot: false,
}

export type SessionScrollRestorePlan =
	| { action: "bottom" }
	| { action: "restore"; scrollTop: number }

const restoringScrollBySessionId = new Set<string>()

export function markSessionScrollRestoring(sessionId: string): void {
	restoringScrollBySessionId.add(sessionId)
}

export function clearSessionScrollRestoring(sessionId: string): void {
	restoringScrollBySessionId.delete(sessionId)
}

export function isRestoringSessionScroll(sessionId: string): boolean {
	return restoringScrollBySessionId.has(sessionId)
}

export function computeScrollAtBottom(element: HTMLElement): boolean {
	const distanceFromBottom =
		element.scrollHeight - element.scrollTop - element.clientHeight
	return distanceFromBottom <= SCROLL_BOTTOM_THRESHOLD_PX
}

export function snapshotFromScrollElement(element: HTMLElement): SessionScrollSnapshot {
	return {
		scrollTop: element.scrollTop,
		atBottom: computeScrollAtBottom(element),
		hasSnapshot: true,
	}
}

export function planSessionScrollRestore(snapshot: SessionScrollSnapshot): SessionScrollRestorePlan {
	if (!snapshot.hasSnapshot || snapshot.atBottom) {
		return { action: "bottom" }
	}
	return { action: "restore", scrollTop: snapshot.scrollTop }
}

export function applySessionScrollRestore(
	element: HTMLElement | null,
	scrollTop: number,
	stopScroll?: () => void,
): void {
	if (!element) return
	element.scrollTop = scrollTop
	stopScroll?.()
}

/** Apply scroll restoration across two animation frames to survive layout shifts. */
export function applySessionScrollRestoreWithRetry(
	getElement: () => HTMLElement | null,
	scrollTop: number,
	stopScroll?: () => void,
): void {
	const apply = () => {
		applySessionScrollRestore(getElement(), scrollTop, stopScroll)
	}
	apply()
	requestAnimationFrame(() => {
		apply()
		requestAnimationFrame(apply)
	})
}

export function isScrollRestoreReady(element: HTMLElement, scrollTop: number): boolean {
	return element.scrollHeight >= scrollTop + element.clientHeight - 1
}

/**
 * Wait until content height can accommodate the target scroll offset, then restore.
 * Falls back to best-effort apply after timeout.
 */
export function restoreSessionScrollWhenReady(args: {
	sessionId: string
	getElement: () => HTMLElement | null
	scrollTop: number
	stopScroll?: () => void
	onRestored?: (scrollTop: number) => void
}): void {
	const { sessionId, getElement, scrollTop, stopScroll, onRestored } = args
	markSessionScrollRestoring(sessionId)
	const startedAt = performance.now()

	const attempt = () => {
		const element = getElement()
		if (!element) {
			clearSessionScrollRestoring(sessionId)
			return
		}
		const ready =
			isScrollRestoreReady(element, scrollTop) ||
			performance.now() - startedAt >= SCROLL_RESTORE_READY_TIMEOUT_MS
		if (!ready) {
			requestAnimationFrame(attempt)
			return
		}
		applySessionScrollRestoreWithRetry(() => element, scrollTop, stopScroll)
		onRestored?.(scrollTop)
		clearSessionScrollRestoring(sessionId)
	}

	requestAnimationFrame(attempt)
}
