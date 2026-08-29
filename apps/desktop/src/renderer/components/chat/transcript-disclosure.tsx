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
	useMemo,
	useState,
	type ReactNode,
} from "react"

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
	const isControlled = openProp !== undefined
	const isOpen = forceOpen || (isControlled ? openProp : uncontrolledOpen)

	const handleOpenChange = useCallback(
		(nextOpen: boolean) => {
			if (forceOpen) return
			if (!isControlled) setUncontrolledOpen(nextOpen)
			onOpenChange?.(nextOpen)
		},
		[forceOpen, isControlled, onOpenChange],
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
 */
export const TranscriptDisclosureContent = memo(function TranscriptDisclosureContent({
	children,
	className,
	rail = false,
}: TranscriptDisclosureContentProps) {
	return (
		<CollapsibleContent
			className="outline-none overflow-hidden"
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
