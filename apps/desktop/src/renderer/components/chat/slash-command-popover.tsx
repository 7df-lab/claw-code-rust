/**
 * Slash command popover — appears when the user types `/` in the input.
 *
 * Desktop intentionally exposes only first-party composer commands here:
 * - /compact executes immediately
 * - /goal and /plan become footer trigger chips
 * - /research stays in the composer so the user can add a research question
 * - Keyboard navigation (Arrow keys, Enter/Tab, Escape)
 */

import { ScrollArea } from "@devo/ui/components/scroll-area"
import { cn } from "@devo/ui/lib/utils"
import fuzzysort from "fuzzysort"
import {
	GoalIcon,
	ListTodoIcon,
	type LucideIcon,
	MicroscopeIcon,
	SparklesIcon,
} from "lucide-react"
import {
	forwardRef,
	memo,
	useCallback,
	useEffect,
	useImperativeHandle,
	useMemo,
	useRef,
	useState,
} from "react"

// ============================================================
// Types
// ============================================================

interface SlashCommand {
	name: string
	description: string
	icon: LucideIcon
	insertText?: string
}

export interface SlashCommandPopoverHandle {
	/** Handle keyboard events from the parent textarea. Returns true if consumed. */
	handleKeyDown: (e: React.KeyboardEvent) => boolean
}

interface SlashCommandPopoverProps {
	/** The query text after `/` */
	query: string
	/** Whether the popover is visible */
	open: boolean
	/** Whether the popover should be active (connected, has session, etc.) */
	enabled: boolean
	/** Callback when a command is selected */
	onSelect: (command: string) => void
	/** Called when Escape is pressed */
	onClose: () => void
}

// ============================================================
// Built-in client commands
// ============================================================

const CLIENT_COMMANDS: SlashCommand[] = [
	{
		name: "compact",
		description: "Summarize conversation to save context",
		icon: SparklesIcon,
	},
	{
		name: "goal",
		description: "Set a goal from the next message",
		icon: GoalIcon,
		insertText: "/goal ",
	},
	{
		name: "plan",
		description: "Create a plan from the next message",
		icon: ListTodoIcon,
		insertText: "/plan ",
	},
	{
		name: "skills",
		description: "Browse and insert a skill",
		icon: SparklesIcon,
	},
	{
		name: "research",
		description: "Run deep research on a question",
		icon: MicroscopeIcon,
		insertText: "/research ",
	},
]

const commandIconClass = "size-3.5 shrink-0 stroke-[1.5] text-muted-foreground"

// ============================================================
// SlashCommandPopover
// ============================================================

export const SlashCommandPopover = memo(
	forwardRef<SlashCommandPopoverHandle, SlashCommandPopoverProps>(function SlashCommandPopover(
		{ query, open, enabled, onSelect, onClose },
		ref,
	) {
		const [activeIndex, setActiveIndex] = useState(0)
		const listRef = useRef<HTMLDivElement>(null)

		// --- Fuzzy filter ---
		const flatList = useMemo<SlashCommand[]>(() => {
			if (!query) return CLIENT_COMMANDS
			const results = fuzzysort.go(query, CLIENT_COMMANDS, {
				keys: ["name", "description"],
				threshold: 0.3,
			})
			return results.map((r) => r.obj)
		}, [query])

		// Reset active index when options or query change
		// biome-ignore lint/correctness/useExhaustiveDependencies: intentional — reset on options/query change
		useEffect(() => {
			setActiveIndex(0)
		}, [flatList.length, query])

		// Scroll active item into view
		// biome-ignore lint/correctness/useExhaustiveDependencies: intentional — scroll when active index changes
		useEffect(() => {
			const list = listRef.current
			if (!list) return
			const active = list.querySelector("[data-active=true]")
			if (active) {
				active.scrollIntoView({ block: "nearest" })
			}
		}, [activeIndex])

		// --- Handle selection ---
		const handleSelect = useCallback(
			(cmd: SlashCommand) => {
				onSelect(cmd.insertText ?? `/${cmd.name}`)
			},
			[onSelect],
		)

		// --- Keyboard handler ---
		const handleKeyDown = useCallback(
			(e: React.KeyboardEvent): boolean => {
				if (!open || !enabled || flatList.length === 0) return false

				switch (e.key) {
					case "ArrowDown": {
						e.preventDefault()
						setActiveIndex((i) => (i + 1) % flatList.length)
						return true
					}
					case "ArrowUp": {
						e.preventDefault()
						setActiveIndex((i) => (i - 1 + flatList.length) % flatList.length)
						return true
					}
					case "Tab":
					case "Enter": {
						e.preventDefault()
						const selected = flatList[activeIndex]
						if (selected) handleSelect(selected)
						return true
					}
					case "Escape": {
						e.preventDefault()
						onClose()
						return true
					}
					default:
						return false
				}
			},
			[open, enabled, flatList, activeIndex, handleSelect, onClose],
		)

		useImperativeHandle(ref, () => ({ handleKeyDown }), [handleKeyDown])

		if (!open || !enabled) return null

		return (
			<div
				role="listbox"
				className="absolute inset-x-0 bottom-full z-50 mb-2 origin-bottom-left overflow-hidden rounded-md border bg-popover shadow-md"
				onMouseDown={(e) => e.preventDefault()}
			>
				{/* User requirement: keep this as a plain command list, without a search/header row. */}
				<ScrollArea className="max-h-72 overflow-hidden [&>[data-slot=scroll-area-viewport]]:max-h-[inherit]">
					<div ref={listRef} className="py-1">
						{flatList.length === 0 && (
							<div className="py-4 text-center text-sm text-muted-foreground">
								No commands found
							</div>
						)}

						{flatList.map((cmd, idx) => (
							<CommandItem
								key={cmd.name}
								command={cmd}
								isActive={idx === activeIndex}
								onSelect={() => handleSelect(cmd)}
								onHover={() => setActiveIndex(idx)}
							/>
						))}
					</div>
				</ScrollArea>
			</div>
		)
	}),
)

// ============================================================
// CommandItem
// ============================================================

const CommandItem = memo(function CommandItem({
	command,
	isActive,
	onSelect,
	onHover,
}: {
	command: SlashCommand
	isActive: boolean
	onSelect: () => void
	onHover: () => void
}) {
	const Icon = command.icon

	return (
		<button
			type="button"
			data-active={isActive}
			className={cn(
				"flex w-full items-center justify-between gap-2 px-3 py-1.5 text-left text-sm transition-colors",
				isActive ? "bg-accent text-accent-foreground" : "hover:bg-muted",
			)}
			onClick={onSelect}
			onMouseEnter={onHover}
		>
			<div className="flex min-w-0 items-center gap-2">
				<Icon className={commandIconClass} aria-hidden="true" />
				<span className="font-medium">/{command.name}</span>
				{command.description && (
					<span className="truncate text-muted-foreground">{command.description}</span>
				)}
			</div>
		</button>
	)
})
