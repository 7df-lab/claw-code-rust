/**
 * @mention popover for Skill, MCP, file, and agent references.
 *
 * Preserves server-ranked references and combines them with local agents.
 */

import type { ReferenceSearchResult } from "@devo-ai/sdk/v2/client"
import fuzzysort from "fuzzysort"
import {
	BrainIcon,
	FileIcon,
	FolderIcon,
	PlugIcon,
	SearchIcon,
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
import { useReferenceSearch } from "../../hooks/use-reference-search"
import type { SdkAgent } from "../../hooks/use-devo-data"
import {
	composerPopoverEmptyClass,
	composerPopoverGroupLabelClass,
	composerPopoverHeaderClass,
	composerPopoverIconClass,
	composerPopoverItemClass,
	composerPopoverListClass,
	composerPopoverScrollClass,
	composerPopoverShellClass,
} from "./composer-popover-styles"

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
		const [activeIndex, setActiveIndex] = useState(0)
		const listRef = useRef<HTMLDivElement>(null)

		// --- Data: agents ---
		const agentOptions = useMemo<MentionOption[]>(
			() =>
				agents
					.filter((a) => !a.hidden && a.mode !== "primary")
					.map((a) => ({ type: "agent" as const, name: a.name, display: a.name })),
			[agents],
		)

		// --- Data: server-ranked Skill, MCP, and File references ---
		const { results, isLoading, error } = useReferenceSearch(directory, query, open)
		const referenceOptions = useMemo(
			() => mapReferenceSearchResults(results).filter(isMentionOptionVisible),
			[results],
		)

		// --- Merge and filter ---
		const allOptions = useMemo<MentionOption[]>(() => {
			if (!query) {
				// No query — show agents + initial references from the server.
				return [...agentOptions, ...referenceOptions]
			}

			// Fuzzy filter agents
			const agentResults = fuzzysort
				.go(query, agentOptions, { key: "display", threshold: 0.3 })
				.map((r) => r.obj)

			// References come pre-filtered and ranked by the server.
			return [...agentResults, ...referenceOptions]
		}, [query, agentOptions, referenceOptions])
		const selectableOptions = useMemo(
			() => allOptions.filter((option) => !isMentionOptionDisabled(option)),
			[allOptions],
		)

		// Reset active index when options or query change
		// biome-ignore lint/correctness/useExhaustiveDependencies: intentional — reset on options/query change
		useEffect(() => {
			setActiveIndex(0)
		}, [allOptions.length, query])

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

		// --- Keyboard handler ---
		const handleKeyDown = useCallback(
			(e: React.KeyboardEvent): boolean => {
				if (!open || selectableOptions.length === 0) return false

				switch (e.key) {
					case "ArrowDown": {
						e.preventDefault()
						setActiveIndex((i) => (i + 1) % selectableOptions.length)
						return true
					}
					case "ArrowUp": {
						e.preventDefault()
						setActiveIndex((i) => (i - 1 + selectableOptions.length) % selectableOptions.length)
						return true
					}
					case "Tab":
					case "Enter": {
						e.preventDefault()
						const selected = selectableOptions[activeIndex]
						if (selected) onSelect(selected)
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
			[open, selectableOptions, activeIndex, onSelect, onClose],
		)

		useImperativeHandle(ref, () => ({ handleKeyDown }), [handleKeyDown])

		if (!open) return null

		// --- Group options ---
		const agentItems = allOptions.filter((option) => option.type === "agent")
		const skillItems = allOptions.filter((option) => option.type === "skill")
		const mcpItems = allOptions.filter((option) => option.type === "mcp")
		const fileItems = allOptions.filter((option) => option.type === "file")
		const hasResults = allOptions.length > 0
		const showLoading = isLoading && !hasResults
		const showError = !!error && !hasResults && !isLoading

		const selectableIndex = (option: MentionOption) => selectableOptions.indexOf(option)

		return (
			<div
				role="listbox"
				className={composerPopoverShellClass}
				onMouseDown={(e) => e.preventDefault()}
			>
				<div className={composerPopoverHeaderClass}>
					<SearchIcon className={composerPopoverIconClass} aria-hidden="true" />
					<span className="min-w-0 truncate text-[13px] leading-5 text-muted-foreground/70">
						{query ? `Searching for “${query}”` : "Mention a file, skill, MCP, or agent"}
					</span>
				</div>

				{/* Single outer scroll only — avoid nested ScrollArea scrollbars. */}
				<div className={composerPopoverScrollClass}>
					<div ref={listRef} className={composerPopoverListClass}>
						{!hasResults && (
							<div className={composerPopoverEmptyClass}>
								{showLoading
									? query
										? `Searching for “${query}”…`
										: "Searching references and agents…"
									: showError
										? error
										: query
											? "No results found"
											: "No references or agents available"}
							</div>
						)}

						{agentItems.length > 0 && (
							<div>
								<div className={composerPopoverGroupLabelClass}>Agents</div>
								{agentItems.map((option) => {
									const idx = selectableIndex(option)
									return (
										<MentionItem
											key={`agent:${option.type === "agent" ? option.name : ""}`}
											option={option}
											isActive={idx === activeIndex}
											onSelect={() => onSelect(option)}
											onHover={() => setActiveIndex(idx)}
										/>
									)
								})}
							</div>
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
							<div>
								<div className={composerPopoverGroupLabelClass}>Files</div>
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
							</div>
						)}
					</div>
				</div>
			</div>
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
		<div>
			<div className={composerPopoverGroupLabelClass}>{label}</div>
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
		</div>
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
			<button
				type="button"
				data-active={isActive}
				className={composerPopoverItemClass(isActive)}
				onClick={onSelect}
				onMouseEnter={onHover}
			>
				<BrainIcon className={composerPopoverIconClass} aria-hidden="true" />
				<span className="font-medium tracking-normal">@{option.name}</span>
			</button>
		)
	}

	if (option.type !== "file") {
		const disabled = option.disabled
		const Icon = option.type === "skill" ? SparklesIcon : PlugIcon
		const detail = option.disabledReason ?? option.description
		return (
			<button
				type="button"
				data-active={isActive}
				disabled={disabled}
				title={detail}
				className={composerPopoverItemClass(isActive, disabled)}
				onClick={onSelect}
				onMouseEnter={onHover}
			>
				<Icon className={composerPopoverIconClass} aria-hidden="true" />
				<span className="shrink-0 font-medium tracking-normal">{option.display}</span>
				{detail && (
					<span className="min-w-0 truncate text-muted-foreground/70">{detail}</span>
				)}
			</button>
		)
	}

	const path = option.path
	const dir = getDirectory(path)
	const name = getFileName(path)
	const isDir = isDirectory(path)

	return (
		<button
			type="button"
			data-active={isActive}
			disabled={option.disabled}
			title={option.disabled ? option.disabledReason : path}
			className={composerPopoverItemClass(isActive, option.disabled)}
			onClick={onSelect}
			onMouseEnter={onHover}
		>
			{isDir ? (
				<FolderIcon className={composerPopoverIconClass} aria-hidden="true" />
			) : (
				<FileIcon className={composerPopoverIconClass} aria-hidden="true" />
			)}
			<div className="flex min-w-0 items-baseline gap-1.5">
				<span className="shrink-0 font-medium tracking-normal">{name}</span>
				{dir && <span className="truncate text-muted-foreground/70">{dir}</span>}
			</div>
		</button>
	)
})
