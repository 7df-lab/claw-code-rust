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
	const latestWidthRef = useRef(width)
	const rafRef = useRef<number | null>(null)

	useEffect(() => {
		latestWidthRef.current = width
	}, [width])

	const endDrag = useCallback(() => {
		if (!dragRef.current) return
		dragRef.current = null
		if (rafRef.current != null) {
			cancelAnimationFrame(rafRef.current)
			rafRef.current = null
		}
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
			latestWidthRef.current = next
			// Coalesce to one React update per frame — sidebar reflow is heavier than the right rail.
			if (rafRef.current != null) return
			rafRef.current = requestAnimationFrame(() => {
				rafRef.current = null
				onWidthChange(latestWidthRef.current)
			})
		},
		[onWidthChange],
	)

	const handlePointerUp = useCallback(
		(event: ReactPointerEvent<HTMLDivElement>) => {
			if (event.currentTarget.hasPointerCapture(event.pointerId)) {
				event.currentTarget.releasePointerCapture(event.pointerId)
			}
			if (rafRef.current != null) {
				cancelAnimationFrame(rafRef.current)
				rafRef.current = null
				onWidthChange(latestWidthRef.current)
			}
			endDrag()
		},
		[endDrag, onWidthChange],
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
				// Hit target stays w-3; paint a thin center line so it matches the right rail
				// and does not flood the Electron titlebar (handle starts below it).
				"absolute bottom-0 z-30 w-3 -translate-x-1/2 cursor-col-resize touch-none bg-transparent",
				"after:pointer-events-none after:absolute after:inset-y-0 after:left-1/2 after:w-0.5 after:-translate-x-1/2 after:rounded-full after:content-['']",
				"hover:after:bg-primary/25 active:after:bg-primary/35",
				className,
			)}
			style={{
				left: "var(--sidebar-width)",
				top: "var(--devo-titlebar-height, 32px)",
			}}
			onPointerDown={handlePointerDown}
			onPointerMove={handlePointerMove}
			onPointerUp={handlePointerUp}
			onPointerCancel={handlePointerUp}
			onDoubleClick={handleDoubleClick}
		/>
	)
}
