import {
	Collapsible,
	CollapsibleContent,
	CollapsibleTrigger,
} from "@devo/ui/components/collapsible"
import { cn } from "@devo/ui/lib/utils"
import { ChevronDownIcon, ChevronRightIcon } from "lucide-react"
import {
	createContext,
	memo,
	useCallback,
	useContext,
	useEffect,
	useLayoutEffect,
	useMemo,
	useRef,
	useState,
	type ReactNode,
} from "react"
import { usePreserveChatScroll } from "../../hooks/use-preserve-chat-scroll"

interface TranscriptDisclosureContextValue {
	isOpen: boolean
	expandable: boolean
}

const TranscriptDisclosureContext = createContext<TranscriptDisclosureContextValue | null>(null)

function useTranscriptDisclosure() {
	const context = useContext(TranscriptDisclosureContext)
	if (!context) {
		throw new Error("Transcript disclosure components must be used within TranscriptDisclosure")
	}
	return context
}

export interface TranscriptDisclosureProps {
	open?: boolean
	defaultOpen?: boolean
	onOpenChange?: (open: boolean) => void
	expandable?: boolean
	forceOpen?: boolean
	className?: string
	children: ReactNode
}

export const TranscriptDisclosure = memo(function TranscriptDisclosure({
	open: openProp,
	defaultOpen = false,
	onOpenChange,
	expandable = true,
	forceOpen = false,
	className,
	children,
}: TranscriptDisclosureProps) {
	const [uncontrolledOpen, setUncontrolledOpen] = useState(defaultOpen)
	const preserveChatScroll = usePreserveChatScroll()
	const isControlled = openProp !== undefined
	const isOpen = forceOpen || (isControlled ? openProp : uncontrolledOpen)

	const handleOpenChange = useCallback(
		(nextOpen: boolean) => {
			if (forceOpen) return
			preserveChatScroll(() => {
				if (!isControlled) setUncontrolledOpen(nextOpen)
				onOpenChange?.(nextOpen)
			})
		},
		[forceOpen, isControlled, onOpenChange, preserveChatScroll],
	)

	const contextValue = useMemo(
		() => ({ expandable: expandable && !forceOpen, isOpen }),
		[expandable, forceOpen, isOpen],
	)

	if (!expandable) {
		return (
			<TranscriptDisclosureContext.Provider value={contextValue}>
				<div className={cn("not-prose", className)}>{children}</div>
			</TranscriptDisclosureContext.Provider>
		)
	}

	return (
		<TranscriptDisclosureContext.Provider value={contextValue}>
			<Collapsible
				className={cn("not-prose", className)}
				open={isOpen}
				onOpenChange={handleOpenChange}
			>
				{children}
			</Collapsible>
		</TranscriptDisclosureContext.Provider>
	)
})

/** Shared process-row trigger: one line height for Thought / tools / groups. */
const triggerClassName =
	"group/row flex w-full max-w-full items-center gap-1.5 rounded-md border-0 bg-transparent px-0 py-0.5 m-0 text-left text-[13px] leading-5 text-muted-foreground transition-colors hover:text-foreground"

export interface TranscriptDisclosureTriggerProps {
	label: ReactNode
	leading?: ReactNode
	trailing?: ReactNode
	className?: string
	"aria-label"?: string
}

export const TranscriptDisclosureTrigger = memo(function TranscriptDisclosureTrigger({
	label,
	leading,
	trailing,
	className,
	"aria-label": ariaLabel,
}: TranscriptDisclosureTriggerProps) {
	const { isOpen, expandable } = useTranscriptDisclosure()
	const ChevronIcon = isOpen ? ChevronDownIcon : ChevronRightIcon

	// Chevron sits immediately after the label. Hidden until hover, keyboard
	// focus, or the row is already open.
	const chevron = expandable ? (
		<ChevronIcon
			aria-hidden="true"
			className={cn(
				"size-3.5 shrink-0 text-muted-foreground/70 transition-opacity",
				isOpen
					? "opacity-100"
					: "opacity-0 group-hover/row:opacity-100 group-focus-visible/row:opacity-100",
			)}
		/>
	) : null
	const trailingSlot = trailing ? (
		<span className="flex shrink-0 items-center">{trailing}</span>
	) : null

	const labelCluster = (
		<span className="flex min-w-0 items-center gap-0.5">
			{leading}
			<span className="min-w-0 truncate">{label}</span>
			{chevron}
		</span>
	)

	if (!expandable) {
		return (
			<div className={cn(triggerClassName, className)} aria-label={ariaLabel}>
				{labelCluster}
				{trailingSlot}
			</div>
		)
	}

	return (
		<CollapsibleTrigger
			className={cn(triggerClassName, className)}
			aria-label={ariaLabel}
		>
			{labelCluster}
			{trailingSlot}
		</CollapsibleTrigger>
	)
})

export interface TranscriptDisclosureContentProps {
	children: ReactNode
	className?: string
	/** Indent content under a left guide line instead of a bordered box. */
	rail?: boolean
}

/**
 * Expanded body for a transcript row.
 * Spacing uses padding inside the panel (not margin) so Base UI's height
 * collapse to 0 does not leave uneven gaps between process rows.
 *
 * Keep `keepMounted={false}` so bodies (especially @pierre/diffs) mount only
 * while open. Skip CSS height transitions — animated 0→N measurement races
 * pierre's virtualizer and blanks the first expand of a session.
 */
export const TranscriptDisclosureContent = memo(function TranscriptDisclosureContent({
	children,
	className,
	rail = false,
}: TranscriptDisclosureContentProps) {
	return (
		<CollapsibleContent
			className="outline-none overflow-hidden [&]:transition-none [&]:animate-none"
			keepMounted={false}
		>
			<div
				className={cn(
					"pt-1",
					rail && "border-l border-border/40 pl-3",
					className,
				)}
			>
				{children}
			</div>
		</CollapsibleContent>
	)
})

const PANEL_READY_MIN_HEIGHT_PX = 8
/** Safety net only — happy path resolves on the next animation frame. */
const PANEL_READY_FALLBACK_MS = 48

function findCollapsiblePanel(node: HTMLElement): HTMLElement | null {
	let current: HTMLElement | null = node
	while (current) {
		if (current.dataset.slot === "collapsible-content") return current
		current = current.parentElement
	}
	return null
}

function isCollapsiblePanelReady(anchor: HTMLElement): boolean {
	const panel = findCollapsiblePanel(anchor)
	if (!panel) return true
	const height = Math.max(panel.getBoundingClientRect().height, panel.scrollHeight)
	return height >= PANEL_READY_MIN_HEIGHT_PX
}

/**
 * Mount disclosure body only after the Base UI collapsible panel has opened in
 * layout. Do not key off the placeholder's own min-height — that fires too
 * early and leaves @pierre/diffs in a 0×0 host (blank first expand).
 */
export const MountWhenVisible = memo(function MountWhenVisible({
	children,
}: {
	children: ReactNode
}) {
	const { isOpen } = useTranscriptDisclosure()
	const anchorRef = useRef<HTMLDivElement>(null)
	const [ready, setReady] = useState(false)

	useLayoutEffect(() => {
		if (!isOpen) {
			setReady(false)
			return
		}

		let cancelled = false
		const anchor = anchorRef.current
		if (!anchor) return

		const markReady = () => {
			if (!cancelled) setReady(true)
		}

		const tryMarkReady = () => {
			if (cancelled || !anchorRef.current) return false
			if (!isCollapsiblePanelReady(anchorRef.current)) return false
			markReady()
			return true
		}

		if (tryMarkReady()) {
			return () => {
				cancelled = true
			}
		}

		const panel = findCollapsiblePanel(anchor)
		const observer =
			typeof ResizeObserver !== "undefined" && panel
				? new ResizeObserver(() => {
						if (tryMarkReady()) observer?.disconnect()
					})
				: null
		observer?.observe(panel ?? anchor)

		const rafId = requestAnimationFrame(() => {
			tryMarkReady()
		})

		const fallbackId = window.setTimeout(() => {
			markReady()
		}, PANEL_READY_FALLBACK_MS)

		return () => {
			cancelled = true
			observer?.disconnect()
			cancelAnimationFrame(rafId)
			clearTimeout(fallbackId)
		}
	}, [isOpen])

	if (!isOpen) return null

	return (
		<div ref={anchorRef} className="w-full">
			{ready ? children : <div className="h-px w-full shrink-0" aria-hidden />}
		</div>
	)
})
