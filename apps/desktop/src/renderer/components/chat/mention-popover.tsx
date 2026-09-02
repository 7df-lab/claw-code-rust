/**
 * @mention popover for Skill, MCP, file, and agent references.
 *
 * Preserves server-ranked references and combines them with local agents.
 * Shares the composer popover chrome with `/` commands.
 */

import type { ReferenceSearchResult } from "@devo-ai/sdk/v2/client"
import fuzzysort from "fuzzysort"
import { BrainIcon, FileIcon, FolderIcon, PlugIcon, SparklesIcon } from "lucide-react"
import { forwardRef, memo, useImperativeHandle, useMemo } from "react"
import { useReferenceSearch } from "../../hooks/use-reference-search"
import type { SdkAgent } from "../../hooks/use-devo-data"
import {
	ComposerPopover,
	ComposerPopoverEmpty,
	ComposerPopoverGroup,
	ComposerPopoverItem,
	composerPopoverHintClass,
	composerPopoverIconClass,
	useComposerPopoverNavigation,
} from "./composer-popover"

// ============================================================
// Types
// ============================================================

export type MentionOption =
	| { type: "agent"; name: string; display: string }
	| {
			type: "file"
			path: string
			display: string
			insertText: string
			disabled: boolean
			disabledReason?: string
	  }
	| {
			type: "skill" | "mcp"
			name: string
			display: string
			description?: string
			insertText: string
			mentionPath?: string
			disabled: boolean
			disabledReason?: string
	  }

export interface MentionPopoverHandle {
	/** Handle keyboard events from the parent textarea. Returns true if consumed. */
	handleKeyDown: (e: React.KeyboardEvent) => boolean
}

interface MentionPopoverProps {
	/** The query text after `@` */
	query: string
	/** Whether the popover is visible */
	open: boolean
	/** Project directory for file search */
	directory: string | null
	/** Available agents */
	agents: SdkAgent[]
	/** Called when a mention is selected */
	onSelect: (option: MentionOption) => void
	/** Called when Escape is pressed */
	onClose: () => void
}

// ============================================================
// Helpers
// ============================================================

function getFileName(path: string): string {
	const parts = path.split("/")
	return parts[parts.length - 1] || path
}

function getDirectory(path: string): string {
	const idx = path.lastIndexOf("/")
	if (idx <= 0) return ""
	return path.slice(0, idx + 1)
}

function isDirectory(path: string): boolean {
	return path.endsWith("/")
}

export function isMentionOptionDisabled(option: MentionOption): boolean {
	return option.type !== "agent" && option.disabled
}

/** Disabled MCPs are omitted from the popover entirely (not shown greyed-out). */
export function isMentionOptionVisible(option: MentionOption): boolean {
	return !(option.type === "mcp" && option.disabled)
}

export function mapReferenceSearchResults(results: ReferenceSearchResult[]): MentionOption[] {
	return results.map((result) => {
		const wire = result as ReferenceSearchResult & {
			isDisabled?: boolean
			disabledReason?: string
		}
		const disabledReason = wire.disabled_reason ?? wire.disabledReason
		const disabled =
			wire.is_disabled === true || wire.isDisabled === true || disabledReason != null
		if (result.kind === "file") {
			return {
				type: "file",
				path: result.mention_path ?? result.display_name,
				display: result.display_name,
				insertText: result.insert_text,
				disabled,
				disabledReason,
			}
		}
		return {
			type: result.kind,
			name: result.display_name,
			display: result.display_name,
			description: result.description,
			insertText: result.insert_text,
			mentionPath: result.mention_path,
			disabled,
			disabledReason,
		}
	})
}

// ============================================================
// MentionPopover
// ============================================================

export const MentionPopover = memo(
	forwardRef<MentionPopoverHandle, MentionPopoverProps>(function MentionPopover(
		{ query, open, directory, agents, onSelect, onClose },
		ref,
	) {
		const agentOptions = useMemo<MentionOption[]>(
			() =>
				agents
					.filter((a) => !a.hidden && a.mode !== "primary")
					.map((a) => ({ type: "agent" as const, name: a.name, display: a.name })),
			[agents],
		)

		const { results, isLoading, error } = useReferenceSearch(directory, query, open)
		const referenceOptions = useMemo(
			() => mapReferenceSearchResults(results).filter(isMentionOptionVisible),
			[results],
		)

		const allOptions = useMemo<MentionOption[]>(() => {
			if (!query) {
				return [...agentOptions, ...referenceOptions]
			}

			const agentResults = fuzzysort
				.go(query, agentOptions, { key: "display", threshold: 0.3 })
				.map((r) => r.obj)

			return [...agentResults, ...referenceOptions]
		}, [query, agentOptions, referenceOptions])
		const selectableOptions = useMemo(
			() => allOptions.filter((option) => !isMentionOptionDisabled(option)),
			[allOptions],
		)

		const { activeIndex, setActiveIndex, listRef, handleKeyDown } = useComposerPopoverNavigation({
			items: selectableOptions,
			open,
			resetKey: query,
			onSelect,
			onClose,
		})

		useImperativeHandle(ref, () => ({ handleKeyDown }), [handleKeyDown])

		if (!open) return null

		const agentItems = allOptions.filter((option) => option.type === "agent")
		const skillItems = allOptions.filter((option) => option.type === "skill")
		const mcpItems = allOptions.filter((option) => option.type === "mcp")
		const fileItems = allOptions.filter((option) => option.type === "file")
		const hasResults = allOptions.length > 0
		const showLoading = isLoading && !hasResults
		const showError = !!error && !hasResults && !isLoading
		const selectableIndex = (option: MentionOption) => selectableOptions.indexOf(option)

		return (
			<ComposerPopover open={open} listRef={listRef}>
				{!hasResults && (
					<ComposerPopoverEmpty>
						{showLoading
							? query
								? `Searching for “${query}”…`
								: "Searching references and agents…"
							: showError
								? error
								: query
									? "No results found"
									: "No references or agents available"}
					</ComposerPopoverEmpty>
				)}

				{agentItems.length > 0 && (
					<MentionGroup
						label="Agents"
						options={agentItems}
						activeIndex={activeIndex}
						selectableIndex={selectableIndex}
						onSelect={onSelect}
						onHover={setActiveIndex}
					/>
				)}

				{skillItems.length > 0 && (
					<MentionGroup
						label="Skills"
						options={skillItems}
						activeIndex={activeIndex}
						selectableIndex={selectableIndex}
						onSelect={onSelect}
						onHover={setActiveIndex}
					/>
				)}

				{mcpItems.length > 0 && (
					<MentionGroup
						label="MCPs"
						options={mcpItems}
						activeIndex={activeIndex}
						selectableIndex={selectableIndex}
						onSelect={onSelect}
						onHover={setActiveIndex}
					/>
				)}

				{fileItems.length > 0 && (
					<ComposerPopoverGroup label="Files">
						{fileItems.map((option) => {
							const idx = selectableIndex(option)
							const path = option.type === "file" ? option.path : ""
							return (
								<MentionItem
									key={`file:${path}`}
									option={option}
									isActive={idx === activeIndex}
									onSelect={() => onSelect(option)}
									onHover={() => {
										if (idx >= 0) setActiveIndex(idx)
									}}
								/>
							)
						})}
					</ComposerPopoverGroup>
				)}
			</ComposerPopover>
		)
	}),
)

const MentionGroup = memo(function MentionGroup({
	label,
	options,
	activeIndex,
	selectableIndex,
	onSelect,
	onHover,
}: {
	label: string
	options: MentionOption[]
	activeIndex: number
	selectableIndex: (option: MentionOption) => number
	onSelect: (option: MentionOption) => void
	onHover: (index: number) => void
}) {
	return (
		<ComposerPopoverGroup label={label}>
			{options.map((option) => {
				const idx = selectableIndex(option)
				return (
					<MentionItem
						key={`${option.type}:${option.type === "agent" ? option.name : option.display}`}
						option={option}
						isActive={idx === activeIndex}
						onSelect={() => onSelect(option)}
						onHover={() => {
							if (idx >= 0) onHover(idx)
						}}
					/>
				)
			})}
		</ComposerPopoverGroup>
	)
})

// ============================================================
// MentionItem
// ============================================================

const MentionItem = memo(function MentionItem({
	option,
	isActive,
	onSelect,
	onHover,
}: {
	option: MentionOption
	isActive: boolean
	onSelect: () => void
	onHover: () => void
}) {
	if (option.type === "agent") {
		return (
			<ComposerPopoverItem isActive={isActive} onSelect={onSelect} onHover={onHover}>
				<BrainIcon className={composerPopoverIconClass} aria-hidden="true" />
				<span className="shrink-0">@{option.name}</span>
			</ComposerPopoverItem>
		)
	}

	if (option.type !== "file") {
		const disabled = option.disabled
		const Icon = option.type === "skill" ? SparklesIcon : PlugIcon
		const detail = option.disabledReason ?? option.description
		return (
			<ComposerPopoverItem
				isActive={isActive}
				disabled={disabled}
				title={detail}
				onSelect={onSelect}
				onHover={onHover}
			>
				<Icon className={composerPopoverIconClass} aria-hidden="true" />
				<span className="shrink-0">{option.display}</span>
				{detail && <span className={composerPopoverHintClass}>{detail}</span>}
			</ComposerPopoverItem>
		)
	}

	const path = option.path
	const dir = getDirectory(path)
	const name = getFileName(path)
	const isDir = isDirectory(path)
	const Icon = isDir ? FolderIcon : FileIcon

	return (
		<ComposerPopoverItem
			isActive={isActive}
			disabled={option.disabled}
			title={option.disabled ? option.disabledReason : path}
			onSelect={onSelect}
			onHover={onHover}
		>
			<Icon className={composerPopoverIconClass} aria-hidden="true" />
			<span className="shrink-0">{name}</span>
			{dir && <span className={composerPopoverHintClass}>{dir}</span>}
		</ComposerPopoverItem>
	)
})
