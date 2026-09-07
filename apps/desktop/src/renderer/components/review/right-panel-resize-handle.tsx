import { cn } from "@devo/ui/lib/utils"
import { useCallback, useEffect, useRef, type PointerEvent as ReactPointerEvent } from "react"
import {
	clampReviewPanelWidth,
	REVIEW_PANEL_DEFAULT_WIDTH_PX,
	REVIEW_PANEL_MAX_WIDTH_PX,
	REVIEW_PANEL_MIN_WIDTH_PX,
} from "../../lib/review-panel-width"

type RightPanelResizeHandleProps = {
	width: number
	onWidthChange: (width: number) => void
	onResizingChange?: (resizing: boolean) => void
	className?: string
}

/**
 * Drag handle on the chat / Changes panel split (left edge of the right rail).
 * Dragging left widens the panel; double-click resets to the default width.
 */
export function RightPanelResizeHandle({
	width,
	onWidthChange,
	onResizingChange,
	className,
}: RightPanelResizeHandleProps) {
	const dragRef = useRef<{ startX: number; startWidth: number } | null>(null)

	const endDrag = useCallback(() => {
		if (!dragRef.current) return
		dragRef.current = null
		onResizingChange?.(false)
		document.body.style.removeProperty("cursor")
		document.body.style.removeProperty("user-select")
	}, [onResizingChange])

	useEffect(() => {
		return () => endDrag()
	}, [endDrag])

	const handlePointerDown = useCallback(
		(event: ReactPointerEvent<HTMLDivElement>) => {
			if (event.button !== 0) return
			event.preventDefault()
			event.currentTarget.setPointerCapture(event.pointerId)
			dragRef.current = { startX: event.clientX, startWidth: width }
			onResizingChange?.(true)
			document.body.style.cursor = "col-resize"
			document.body.style.userSelect = "none"
		},
		[onResizingChange, width],
	)

	const handlePointerMove = useCallback(
		(event: ReactPointerEvent<HTMLDivElement>) => {
			const drag = dragRef.current
			if (!drag) return
			// Left edge: moving pointer left → wider panel.
			const next = clampReviewPanelWidth(drag.startWidth + (drag.startX - event.clientX), {
				windowWidth: window.innerWidth,
			})
			onWidthChange(next)
		},
		[onWidthChange],
	)

	const handlePointerUp = useCallback(
		(event: ReactPointerEvent<HTMLDivElement>) => {
			if (event.currentTarget.hasPointerCapture(event.pointerId)) {
				event.currentTarget.releasePointerCapture(event.pointerId)
			}
			endDrag()
		},
		[endDrag],
	)

	const handleDoubleClick = useCallback(() => {
		onWidthChange(
			clampReviewPanelWidth(REVIEW_PANEL_DEFAULT_WIDTH_PX, { windowWidth: window.innerWidth }),
		)
	}, [onWidthChange])

	return (
		<div
			role="separator"
			aria-orientation="vertical"
			aria-label="Resize changes panel"
			aria-valuemin={REVIEW_PANEL_MIN_WIDTH_PX}
			aria-valuemax={REVIEW_PANEL_MAX_WIDTH_PX}
			aria-valuenow={width}
			data-slot="right-panel-resize-handle"
			className={cn(
				"absolute inset-y-0 left-0 z-40 w-3 -translate-x-1/2 cursor-col-resize touch-none bg-transparent",
				"after:pointer-events-none after:absolute after:inset-y-0 after:left-1/2 after:w-0.5 after:-translate-x-1/2 after:rounded-full after:content-['']",
				"hover:after:bg-primary/25 active:after:bg-primary/35",
				className,
			)}
			onPointerDown={handlePointerDown}
			onPointerMove={handlePointerMove}
			onPointerUp={handlePointerUp}
			onPointerCancel={handlePointerUp}
			onDoubleClick={handleDoubleClick}
		/>
	)
}
