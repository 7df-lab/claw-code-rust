/**
 * Skill picker dialog — opened via /skills slash command.
 *
 * Matches the Devo TUI pattern: a dedicated dialog for browsing
 * and selecting skills, separate from the main slash command popover.
 * Uses the SDK's app.skills() endpoint for the full skill list.
 */

import {
	Dialog,
	DialogContent,
	DialogDescription,
	DialogHeader,
	DialogTitle,
} from "@devo/ui/components/dialog"
import { Input } from "@devo/ui/components/input"
import { ScrollArea } from "@devo/ui/components/scroll-area"
import { cn } from "@devo/ui/lib/utils"
import { useQuery } from "@tanstack/react-query"
import fuzzysort from "fuzzysort"
import { BookOpenIcon, SearchIcon } from "lucide-react"
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { getProjectClient } from "../../services/connection-manager"

// ============================================================
// Types
// ============================================================

interface Skill {
	name: string
	description: string
	location: string
}

interface SkillPickerDialogProps {
	open: boolean
	onOpenChange: (open: boolean) => void
	/** Project directory for fetching skills */
	directory: string | null
	/** Called when a skill is selected — inserts `/skillname ` into input */
	onSelect: (skillName: string) => void
}

// ============================================================
// Hook: useSkills
// ============================================================

function useSkills(directory: string | null, enabled: boolean) {
	const { data, isLoading } = useQuery({
		queryKey: ["skills", directory],
		queryFn: async () => {
			const client = getProjectClient(directory!)
			if (!client) return []
			const result = await client.app.skills()
			return ((result.data ?? []) as Array<Record<string, unknown>>)
				.filter((skill) => skill.enabled !== false)
				.map((skill) => ({
					name: String(skill.name ?? ""),
					description: String(
						skill.shortDescription ?? skill.short_description ?? skill.description ?? "",
					),
					location: String(skill.path ?? skill.location ?? ""),
				})) as Skill[]
		},
		enabled: !!directory && enabled,
		staleTime: 30_000,
	})

	return { skills: data ?? [], isLoading }
}

// ============================================================
// SkillPickerDialog
// ============================================================

export const SkillPickerDialog = memo(function SkillPickerDialog({
	open,
	onOpenChange,
	directory,
	onSelect,
}: SkillPickerDialogProps) {
	const [search, setSearch] = useState("")
	const [activeIndex, setActiveIndex] = useState(0)
	const searchRef = useRef<HTMLInputElement>(null)
	const listRef = useRef<HTMLDivElement>(null)

	const { skills, isLoading } = useSkills(directory, open)

	// Fuzzy filter
	const filtered = useMemo<Skill[]>(() => {
		if (!search) return skills
		const results = fuzzysort.go(search, skills, {
			keys: ["name", "description"],
			threshold: 0.3,
		})
		return results.map((r) => r.obj)
	}, [skills, search])

	// Reset state when dialog opens
	useEffect(() => {
		if (open) {
			setSearch("")
			setActiveIndex(0)
			// Focus search input after animation
			requestAnimationFrame(() => {
				searchRef.current?.focus()
			})
		}
	}, [open])

	// Reset active index on filter change
	// biome-ignore lint/correctness/useExhaustiveDependencies: intentional — reset on filter change
	useEffect(() => {
		setActiveIndex(0)
	}, [filtered.length, search])

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

	const handleSelect = useCallback(
		(skill: Skill) => {
			onSelect(skill.name)
			onOpenChange(false)
		},
		[onSelect, onOpenChange],
	)

	const handleKeyDown = useCallback(
		(e: React.KeyboardEvent) => {
			if (filtered.length === 0) return

			switch (e.key) {
				case "ArrowDown": {
					e.preventDefault()
					setActiveIndex((i) => (i + 1) % filtered.length)
					break
				}
				case "ArrowUp": {
					e.preventDefault()
					setActiveIndex((i) => (i - 1 + filtered.length) % filtered.length)
					break
				}
				case "Enter": {
					e.preventDefault()
					const selected = filtered[activeIndex]
					if (selected) handleSelect(selected)
					break
				}
			}
		},
		[filtered, activeIndex, handleSelect],
	)

	return (
		<Dialog open={open} onOpenChange={onOpenChange}>
			<DialogContent className="max-w-lg gap-0 p-0" showCloseButton={false}>
				<DialogHeader className="p-4 pb-0">
					<DialogTitle className="flex items-center gap-2 text-base">
						<BookOpenIcon className="size-4" />
						Skills
					</DialogTitle>
					<DialogDescription>Select a skill to use in your prompt</DialogDescription>
				</DialogHeader>

				{/* Search */}
				<div className="relative px-4 pt-3">
					<SearchIcon className="absolute left-7 top-1/2 mt-1.5 size-4 -translate-y-1/2 text-muted-foreground" />
					<Input
						ref={searchRef}
						value={search}
						onChange={(e) => setSearch(e.target.value)}
						onKeyDown={handleKeyDown}
						placeholder="Search skills..."
						className="pl-9"
					/>
				</div>

				{/* Skills list */}
				<ScrollArea className="max-h-72 px-2 py-2 [&>[data-slot=scroll-area-viewport]]:max-h-[inherit]">
					<div ref={listRef}>
						{isLoading && (
							<div className="py-8 text-center text-sm text-muted-foreground">
								Loading skills...
							</div>
						)}

						{!isLoading && filtered.length === 0 && (
							<div className="py-8 text-center text-sm text-muted-foreground">
								{search ? "No skills found" : "No skills available"}
							</div>
						)}

						{filtered.map((skill, idx) => (
							<button
								key={skill.name}
								type="button"
								data-active={idx === activeIndex}
								className={cn(
									"flex w-full flex-col gap-0.5 rounded-md px-3 py-2 text-left transition-colors",
									idx === activeIndex ? "bg-accent text-accent-foreground" : "hover:bg-muted",
								)}
								onClick={() => handleSelect(skill)}
								onMouseEnter={() => setActiveIndex(idx)}
							>
								<span className="text-sm font-medium">{skill.name}</span>
								{skill.description && (
									<span className="line-clamp-2 text-xs text-muted-foreground">
										{skill.description}
									</span>
								)}
							</button>
						))}
					</div>
				</ScrollArea>
			</DialogContent>
		</Dialog>
	)
})
