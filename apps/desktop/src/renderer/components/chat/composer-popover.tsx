/**
 * Shared composer suggestion popover used by `@` mentions and `/` commands.
 *
 * Filtering happens in the composer itself — this panel is a compact list,
 * matching the app's option-menu chrome (Cursor / Codex style).
 */

import {
	optionMenuContentClass,
	optionMenuItemClass,
} from "@devo/ui/components/option-menu-styles"
import { cn } from "@devo/ui/lib/utils"
import {
	type KeyboardEvent,
	type ReactNode,
	type Ref,
	memo,
	useCallback,
	useEffect,
	useRef,
	useState,
} from "react"

export const composerPopoverShellClass = cn(
	"absolute inset-x-0 bottom-full z-50 mb-1.5 origin-bottom overflow-hidden text-popover-foreground",
	optionMenuContentClass,
)

export const composerPopoverScrollClass = "max-h-64 overflow-y-auto overscroll-contain"

export const composerPopoverListClass = "flex flex-col"

export const composerPopoverGroupLabelClass =
	"sticky top-0 z-10 bg-popover px-2 py-1 text-[11px] font-medium text-muted-foreground"

export const composerPopoverEmptyClass =
	"px-2 py-6 text-center text-[13px] leading-5 text-muted-foreground"

export const composerPopoverIconClass = "size-3.5 shrink-0 stroke-[1.5] text-muted-foreground"

export const composerPopoverHintClass = "min-w-0 truncate text-muted-foreground"

export function composerPopoverItemClass(isActive: boolean, disabled = false): string {
	return cn(
		"flex w-full items-center text-left transition-colors",
		optionMenuItemClass,
		isActive ? "bg-muted text-foreground" : "hover:bg-muted/70",
		disabled && "cursor-not-allowed opacity-45 hover:bg-transparent",
	)
}

export function ComposerPopover({
	open,
	listRef,
	children,
}: {
	open: boolean
	listRef?: Ref<HTMLDivElement>
	children: ReactNode
}) {
	if (!open) return null

	return (
		<div
			role="listbox"
			className={composerPopoverShellClass}
			onMouseDown={(event) => event.preventDefault()}
		>
			<div className={composerPopoverScrollClass}>
				<div ref={listRef} className={composerPopoverListClass}>
					{children}
				</div>
			</div>
		</div>
	)
}

export const ComposerPopoverGroup = memo(function ComposerPopoverGroup({
	label,
	children,
}: {
	label: string
	children: ReactNode
}) {
	return (
		<div>
			<div className={composerPopoverGroupLabelClass}>{label}</div>
			{children}
		</div>
	)
})

export const ComposerPopoverItem = memo(function ComposerPopoverItem({
	isActive,
	disabled = false,
	title,
	onSelect,
	onHover,
	children,
}: {
	isActive: boolean
	disabled?: boolean
	title?: string
	onSelect: () => void
	onHover: () => void
	children: ReactNode
}) {
	return (
		<button
			type="button"
			role="option"
			aria-selected={isActive}
			data-active={isActive}
			disabled={disabled}
			title={title}
			className={composerPopoverItemClass(isActive, disabled)}
			onClick={onSelect}
			onMouseEnter={onHover}
		>
			{children}
		</button>
	)
})

export function ComposerPopoverEmpty({ children }: { children: ReactNode }) {
	return <div className={composerPopoverEmptyClass}>{children}</div>
}

export function useComposerPopoverNavigation<T>({
	items,
	open,
	enabled = true,
	resetKey,
	onSelect,
	onClose,
}: {
	items: T[]
	open: boolean
	enabled?: boolean
	resetKey: string
	onSelect: (item: T) => void
	onClose: () => void
}) {
	const [activeIndex, setActiveIndex] = useState(0)
	const listRef = useRef<HTMLDivElement>(null)

	// biome-ignore lint/correctness/useExhaustiveDependencies: reset when the query or result count changes
	useEffect(() => {
		setActiveIndex(0)
	}, [items.length, resetKey])

	// biome-ignore lint/correctness/useExhaustiveDependencies: scroll when the highlighted row changes
	useEffect(() => {
		const list = listRef.current
		if (!list) return
		const active = list.querySelector("[data-active=true]")
		if (active) {
			active.scrollIntoView({ block: "nearest" })
		}
	}, [activeIndex])

	const handleKeyDown = useCallback(
		(event: KeyboardEvent): boolean => {
			if (!open || !enabled || items.length === 0) return false

			switch (event.key) {
				case "ArrowDown": {
					event.preventDefault()
					setActiveIndex((index) => (index + 1) % items.length)
					return true
				}
				case "ArrowUp": {
					event.preventDefault()
					setActiveIndex((index) => (index - 1 + items.length) % items.length)
					return true
				}
				case "Tab":
				case "Enter": {
					event.preventDefault()
					const selected = items[activeIndex]
					if (selected) onSelect(selected)
					return true
				}
				case "Escape": {
					event.preventDefault()
					onClose()
					return true
				}
				default:
					return false
			}
		},
		[open, enabled, items, activeIndex, onSelect, onClose],
	)

	return { activeIndex, setActiveIndex, listRef, handleKeyDown }
}
