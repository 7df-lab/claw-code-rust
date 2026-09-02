/**
 * Slash command popover — appears when the user types `/` in the input.
 *
 * Desktop intentionally exposes only first-party composer commands here:
 * - /compact executes immediately
 * - /goal becomes a footer trigger chip
 * - /plan switches to plan mode (footer badge only while plan mode is active)
 * - /research stays in the composer so the user can add a research question
 * - Keyboard navigation (Arrow keys, Enter/Tab, Escape)
 *
 * Shares the composer popover chrome with `@` mentions.
 */

import fuzzysort from "fuzzysort"
import {
	GitBranchIcon,
	GoalIcon,
	ListTodoIcon,
	type LucideIcon,
	MessageCircleQuestionIcon,
	MicroscopeIcon,
	SparklesIcon,
} from "lucide-react"
import { forwardRef, memo, useCallback, useImperativeHandle, useMemo } from "react"
import {
	ComposerPopover,
	ComposerPopoverEmpty,
	ComposerPopoverItem,
	composerPopoverHintClass,
	composerPopoverIconClass,
	useComposerPopoverNavigation,
} from "./composer-popover"

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
		name: "fork",
		description: "Fork this session into a new branch",
		icon: GitBranchIcon,
	},
	{
		name: "side",
		description: "Ask a one-turn side question (btw)",
		icon: MessageCircleQuestionIcon,
		insertText: "/side ",
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

/** Kept for sidebar / chip icon-stroke parity assertions. */
const commandIconClass = composerPopoverIconClass

// ============================================================
// SlashCommandPopover
// ============================================================

export const SlashCommandPopover = memo(
	forwardRef<SlashCommandPopoverHandle, SlashCommandPopoverProps>(function SlashCommandPopover(
		{ query, open, enabled, onSelect, onClose },
		ref,
	) {
		const flatList = useMemo<SlashCommand[]>(() => {
			if (!query) return CLIENT_COMMANDS
			return fuzzysort
				.go(query, CLIENT_COMMANDS, {
					keys: ["name", "description"],
					threshold: 0.3,
				})
				.map((result) => result.obj)
		}, [query])

		const handleSelect = useCallback(
			(cmd: SlashCommand) => {
				onSelect(cmd.insertText ?? `/${cmd.name}`)
			},
			[onSelect],
		)

		const { activeIndex, setActiveIndex, listRef, handleKeyDown } = useComposerPopoverNavigation({
			items: flatList,
			open,
			enabled,
			resetKey: query,
			onSelect: handleSelect,
			onClose,
		})

		useImperativeHandle(ref, () => ({ handleKeyDown }), [handleKeyDown])

		return (
			<ComposerPopover open={open && enabled} listRef={listRef}>
				{flatList.length === 0 && <ComposerPopoverEmpty>No commands found</ComposerPopoverEmpty>}

				{flatList.map((cmd, idx) => (
					<CommandItem
						key={cmd.name}
						command={cmd}
						isActive={idx === activeIndex}
						onSelect={() => handleSelect(cmd)}
						onHover={() => setActiveIndex(idx)}
					/>
				))}
			</ComposerPopover>
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
		<ComposerPopoverItem isActive={isActive} onSelect={onSelect} onHover={onHover}>
			<Icon className={commandIconClass} aria-hidden="true" />
			<span className="shrink-0">/{command.name}</span>
			{command.description && (
				<span className={composerPopoverHintClass}>{command.description}</span>
			)}
		</ComposerPopoverItem>
	)
})
