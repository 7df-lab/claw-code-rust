import { Popover as PopoverPrimitive } from "@base-ui/react/popover"
import { cn } from "@devo/ui/lib/utils"
import {
	createContext,
	type ReactNode,
	useCallback,
	useContext,
	useEffect,
	useRef,
	useState,
} from "react"
import { optionMenuContentClass, optionMenuItemClass } from "./option-menu-styles"
import { ScrollArea } from "./scroll-area"

// ============================================================
// Context — shared open/search state
// ============================================================

interface SearchableListPopoverContextValue {
	open: boolean
	setOpen: (open: boolean) => void
	search: string
	setSearch: (search: string) => void
}

const SearchableListPopoverContext = createContext<SearchableListPopoverContextValue | null>(null)

function useSearchableListPopover() {
	const ctx = useContext(SearchableListPopoverContext)
	if (!ctx) throw new Error("SearchableListPopover.* must be used within <SearchableListPopover>")
	return ctx
}

// ============================================================
// Root — manages popover + search state
// ============================================================

interface SearchableListPopoverProps {
	children: ReactNode
	open?: boolean
	onOpenChange?: (open: boolean) => void
}

function SearchableListPopover({
	children,
	open: controlledOpen,
	onOpenChange,
}: SearchableListPopoverProps) {
	const [internalOpen, setInternalOpen] = useState(false)
	const open = controlledOpen ?? internalOpen

	const setOpen = useCallback(
		(next: boolean) => {
			if (controlledOpen === undefined) setInternalOpen(next)
			onOpenChange?.(next)
		},
		[controlledOpen, onOpenChange],
	)

	const [search, setSearch] = useState("")

	// Reset search when popover closes
	useEffect(() => {
		if (!open) setSearch("")
	}, [open])

	return (
		<SearchableListPopoverContext.Provider value={{ open, setOpen, search, setSearch }}>
			<PopoverPrimitive.Root open={open} onOpenChange={setOpen}>
				{children}
			</PopoverPrimitive.Root>
		</SearchableListPopoverContext.Provider>
	)
}

// ============================================================
// Trigger
// ============================================================

function SearchableListPopoverTrigger({ className, ...props }: PopoverPrimitive.Trigger.Props) {
	return (
		<PopoverPrimitive.Trigger
			data-slot="searchable-list-popover-trigger"
			className={className}
			{...props}
		/>
	)
}

// ============================================================
// Content — the popover panel with proper sizing
// ============================================================

interface SearchableListPopoverContentProps extends PopoverPrimitive.Popup.Props {
	/** Width class, defaults to "w-72" */
	width?: string
	align?: PopoverPrimitive.Positioner.Props["align"]
	side?: PopoverPrimitive.Positioner.Props["side"]
	sideOffset?: PopoverPrimitive.Positioner.Props["sideOffset"]
}

function SearchableListPopoverContent({
	children,
	className,
	width = "w-72",
	align = "end",
	side = "top",
	sideOffset = 4,
	...props
}: SearchableListPopoverContentProps) {
	return (
		<PopoverPrimitive.Portal>
			<PopoverPrimitive.Positioner
				align={align}
				side={side}
				sideOffset={sideOffset}
				className="isolate z-50"
			>
				<PopoverPrimitive.Popup
					data-slot="searchable-list-popover-content"
					className={cn(
						"text-popover-foreground data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 z-50 origin-(--transform-origin) overflow-hidden outline-hidden",
						optionMenuContentClass,
						width,
						className,
					)}
					{...props}
				>
					{children}
				</PopoverPrimitive.Popup>
			</PopoverPrimitive.Positioner>
		</PopoverPrimitive.Portal>
	)
}

// ============================================================
// Search — auto-focused search input
// ============================================================

interface SearchableListPopoverSearchProps {
	placeholder?: string
	icon?: ReactNode
	className?: string
}

function SearchableListPopoverSearch({
	placeholder = "Search...",
	icon,
	className,
}: SearchableListPopoverSearchProps) {
	const { open, search, setSearch } = useSearchableListPopover()
	const inputRef = useRef<HTMLInputElement>(null)

	useEffect(() => {
		if (open) {
			const timer = setTimeout(() => inputRef.current?.focus(), 0)
			return () => clearTimeout(timer)
		}
	}, [open])

	return (
		<div className={cn("flex items-center border-b px-3 py-2", className)}>
			{icon && <span className="mr-2 flex shrink-0 text-muted-foreground">{icon}</span>}
			<input
				ref={inputRef}
				type="text"
				value={search}
				onChange={(e) => setSearch(e.target.value)}
				placeholder={placeholder}
				className="h-6 w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground/50"
			/>
		</div>
	)
}

// ============================================================
// List — scrollable container with proper max-height
// ============================================================

interface SearchableListPopoverListProps {
	children: ReactNode
	className?: string
	/** Max height class, defaults to "max-h-64" */
	maxHeight?: string
}

function SearchableListPopoverList({
	children,
	className,
	maxHeight = "max-h-64",
}: SearchableListPopoverListProps) {
	return (
		<ScrollArea
			className={cn(
				"overflow-hidden [&>[data-slot=scroll-area-viewport]]:max-h-[inherit]",
				maxHeight,
				className,
			)}
		>
			{children}
		</ScrollArea>
	)
}

// ============================================================
// Group — section with a sticky header
// ============================================================

interface SearchableListPopoverGroupProps {
	label: React.ReactNode
	children: ReactNode
	className?: string
}

function SearchableListPopoverGroup({
	label,
	children,
	className,
}: SearchableListPopoverGroupProps) {
	return (
		<div className={className}>
			<div className="sticky top-0 z-10 flex items-center gap-2 bg-popover px-2 py-1.5 text-[11px] font-medium text-muted-foreground">
				{label}
			</div>
			{children}
		</div>
	)
}

// ============================================================
// Item — a single selectable row
// ============================================================

interface SearchableListPopoverItemProps {
	children: ReactNode
	onSelect: () => void
	isActive?: boolean
	className?: string
}

function SearchableListPopoverItem({
	children,
	onSelect,
	isActive,
	className,
}: SearchableListPopoverItemProps) {
	return (
		<button
			type="button"
			onClick={onSelect}
			className={cn(
				"flex w-full min-w-0 items-center text-left transition-colors hover:bg-muted hover:text-foreground",
				optionMenuItemClass,
				isActive && "bg-muted text-foreground",
				className,
			)}
		>
			{children}
		</button>
	)
}

// ============================================================
// Empty — fallback when list is empty
// ============================================================

interface SearchableListPopoverEmptyProps {
	children?: ReactNode
	className?: string
}

function SearchableListPopoverEmpty({ children, className }: SearchableListPopoverEmptyProps) {
	return (
		<div className={cn("px-2 py-6 text-center text-[13px] text-muted-foreground", className)}>
			{children ?? "No results found"}
		</div>
	)
}

// ============================================================
// Hook — expose search value for consumer filtering
// ============================================================

function useSearchableListPopoverSearch() {
	const { search } = useSearchableListPopover()
	return search
}

// ============================================================
// Exports
// ============================================================

export {
	SearchableListPopover,
	SearchableListPopoverTrigger,
	SearchableListPopoverContent,
	SearchableListPopoverSearch,
	SearchableListPopoverList,
	SearchableListPopoverGroup,
	SearchableListPopoverItem,
	SearchableListPopoverEmpty,
	useSearchableListPopoverSearch,
}
