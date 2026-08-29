import { useSidebar } from "@devo/ui/components/sidebar"
import { cn } from "@devo/ui/lib/utils"
import { useCallback, useEffect, useRef, type PointerEvent as ReactPointerEvent } from "react"
import {
	clampSidebarWidth,
	SIDEBAR_DEFAULT_WIDTH_PX,
	SIDEBAR_MAX_WIDTH_PX,
	SIDEBAR_MIN_WIDTH_PX,
} from "../../lib/sidebar-width"

type SidebarResizeHandleProps = {
	width: number
	onWidthChange: (width: number) => void
	onResizingChange?: (resizing: boolean) => void
	className?: string
}

/**
 * Drag handle on the sidebar / main content split.
 * Double-click resets to the default width.
 */
export function SidebarResizeHandle({
	width,
	onWidthChange,
	onResizingChange,
	className,
}: SidebarResizeHandleProps) {
	const { open } = useSidebar()
	const dragRef = useRef<{ startX: number; startWidth: number } | null>(null)

	const endDrag = useCallback(() => {
		if (!dragRef.current) return
		dragRef.current = null
		onResizingChange?.(false)
		document.body.style.removeProperty("cursor")
		document.body.style.removeProperty("user-select")
	}, [onResizingChange])

	useEffect(() => {
		if (!open) endDrag()
	}, [endDrag, open])

	useEffect(() => {
		return () => endDrag()
	}, [endDrag])

	const handlePointerDown = useCallback(
		(event: ReactPointerEvent<HTMLDivElement>) => {
			if (event.button !== 0 || !open) return
			event.preventDefault()
			event.currentTarget.setPointerCapture(event.pointerId)
			dragRef.current = { startX: event.clientX, startWidth: width }
			onResizingChange?.(true)
			document.body.style.cursor = "col-resize"
			document.body.style.userSelect = "none"
		},
		[onResizingChange, open, width],
	)

	const handlePointerMove = useCallback(
		(event: ReactPointerEvent<HTMLDivElement>) => {
			const drag = dragRef.current
			if (!drag) return
			const next = clampSidebarWidth(drag.startWidth + (event.clientX - drag.startX), {
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
			clampSidebarWidth(SIDEBAR_DEFAULT_WIDTH_PX, { windowWidth: window.innerWidth }),
		)
	}, [onWidthChange])

	if (!open) return null

	return (
		<div
			role="separator"
			aria-orientation="vertical"
			aria-label="Resize sidebar"
			aria-valuemin={SIDEBAR_MIN_WIDTH_PX}
			aria-valuemax={SIDEBAR_MAX_WIDTH_PX}
			aria-valuenow={width}
			data-slot="sidebar-resize-handle"
			className={cn(
				"absolute inset-y-0 z-40 w-3 -translate-x-1/2 cursor-col-resize touch-none bg-transparent",
				className,
			)}
			style={{ left: "var(--sidebar-width)" }}
			onPointerDown={handlePointerDown}
			onPointerMove={handlePointerMove}
			onPointerUp={handlePointerUp}
			onPointerCancel={handlePointerUp}
			onDoubleClick={handleDoubleClick}
		/>
	)
}
