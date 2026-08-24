import {
	optionMenuContentClass,
	optionMenuIconClass,
	optionMenuItemClass,
	optionMenuSeparatorClass,
} from "@devo/ui/components/option-menu-styles"
import { Popover, PopoverContent, PopoverTrigger } from "@devo/ui/components/popover"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { cn } from "@devo/ui/lib/utils"
import {
	ChevronRightIcon,
	FolderIcon,
	NotebookTextIcon,
	PlusIcon,
	SearchIcon,
	XIcon,
} from "lucide-react"
import { useMemo, useState } from "react"
import type { SidebarProject } from "../lib/types"

function projectNameFromDirectory(directory: string): string {
	const trimmed = directory.replace(/[\\/]+$/, "")
	if (!trimmed) return "Project"
	return trimmed.split(/[\\/]/).filter(Boolean).at(-1) ?? trimmed
}

export function NewChatProjectPicker({
	projects,
	selectedProject,
	selectedDirectory,
	onSelectProject,
	onClearProject,
	onStartFromScratch,
	onUseExistingFolder,
}: {
	projects: SidebarProject[]
	selectedProject: SidebarProject | undefined
	selectedDirectory: string
	onSelectProject: (project: SidebarProject) => void
	onClearProject?: () => void
	onStartFromScratch?: () => void | Promise<void>
	onUseExistingFolder?: () => Promise<void>
}) {
	const [open, setOpen] = useState(false)
	const [newProjectOpen, setNewProjectOpen] = useState(false)
	const [query, setQuery] = useState("")
	const displayName =
		selectedProject?.name ?? (selectedDirectory ? projectNameFromDirectory(selectedDirectory) : "")
	const filteredProjects = useMemo(() => {
		const normalizedQuery = query.trim().toLowerCase()
		if (!normalizedQuery) return projects
		return projects.filter(
			(project) =>
				project.name.toLowerCase().includes(normalizedQuery) ||
				project.directory.toLowerCase().includes(normalizedQuery),
		)
	}, [projects, query])

	const trigger = displayName ? (
		<div className="group relative inline-flex h-8 items-center">
			<PopoverTrigger
				render={
					<button
						type="button"
						className="flex h-8 max-w-[360px] items-center gap-2 rounded-full px-3 text-sm text-foreground transition-colors hover:bg-black/[0.08] dark:hover:bg-white/[0.10]"
					/>
				}
			>
				<NotebookTextIcon className="size-4 shrink-0 text-muted-foreground transition-opacity group-hover:opacity-0" />
				<span className="min-w-0 truncate">{displayName}</span>
			</PopoverTrigger>
			{onClearProject ? (
				<Tooltip>
					<TooltipTrigger
						render={
							<button
								type="button"
								aria-label="Clear project"
								className="absolute left-2 flex size-5 items-center justify-center rounded-full bg-muted-foreground text-background opacity-0 transition-opacity group-hover:opacity-100"
								onClick={(event) => {
									event.stopPropagation()
									onClearProject()
								}}
							/>
						}
					>
						<XIcon className="size-3" />
					</TooltipTrigger>
					<TooltipContent>Change project</TooltipContent>
				</Tooltip>
			) : null}
		</div>
	) : (
		<PopoverTrigger
			render={
				<button
					type="button"
					className="flex h-8 items-center gap-2 rounded-full px-3 text-sm text-foreground transition-colors hover:bg-black/[0.06] dark:hover:bg-white/[0.08]"
				/>
			}
		>
			<NotebookTextIcon className="size-4 shrink-0 text-muted-foreground" />
			<span>Choose project</span>
		</PopoverTrigger>
	)

	return (
		<Popover open={open} onOpenChange={setOpen}>
			{trigger}
			<PopoverContent
				align="start"
				side="top"
				sideOffset={8}
				className={cn(optionMenuContentClass, "w-[220px] max-w-[calc(100vw-32px)] gap-0")}
			>
				<div className="flex h-8 items-center gap-2 px-2 text-muted-foreground">
					<SearchIcon className={optionMenuIconClass} />
					<input
						value={query}
						onChange={(event) => setQuery(event.target.value)}
						placeholder="Search projects"
						className="h-full min-w-0 flex-1 bg-transparent text-[13px] text-foreground placeholder:text-muted-foreground focus:outline-none"
					/>
				</div>
				<div className="flex max-h-[220px] flex-col overflow-auto">
					{filteredProjects.map((project) => (
						<button
							key={project.directory}
							type="button"
							onClick={() => {
								onSelectProject(project)
								setOpen(false)
							}}
							className={cn(
								"flex w-full items-center text-left transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none",
								optionMenuItemClass,
								project.directory === selectedDirectory && "bg-black/[0.06] dark:bg-white/[0.08]",
							)}
						>
							<NotebookTextIcon className={optionMenuIconClass} />
							<span className="min-w-0 flex-1 truncate">{project.name}</span>
						</button>
					))}
				</div>
				<div className={cn("border-t border-border/70 pt-1", optionMenuSeparatorClass)}>
					<Popover open={newProjectOpen} onOpenChange={setNewProjectOpen}>
						<PopoverTrigger
							render={
								<button
									type="button"
									className={cn(
										"flex w-full items-center text-left transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none",
										optionMenuItemClass,
									)}
								/>
							}
						>
							<PlusIcon className={optionMenuIconClass} />
							<span className="min-w-0 flex-1 truncate">New project</span>
							<ChevronRightIcon className={optionMenuIconClass} />
						</PopoverTrigger>
						<PopoverContent
							align="start"
							side="right"
							sideOffset={10}
							className={cn(optionMenuContentClass, "w-[210px] gap-0")}
						>
							<button
								type="button"
								disabled={!onStartFromScratch}
								onClick={() => {
									setNewProjectOpen(false)
									setOpen(false)
									void onStartFromScratch?.()
								}}
								className={cn(
									"flex w-full items-center text-left transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:opacity-45",
									optionMenuItemClass,
								)}
							>
								<PlusIcon className={optionMenuIconClass} />
								<span className="min-w-0 flex-1 truncate">Start from scratch</span>
							</button>
							<button
								type="button"
								disabled={!onUseExistingFolder}
								onClick={() => {
									setNewProjectOpen(false)
									setOpen(false)
									void onUseExistingFolder?.()
								}}
								className={cn(
									"flex w-full items-center text-left transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none disabled:opacity-45",
									optionMenuItemClass,
								)}
							>
								<FolderIcon className={optionMenuIconClass} />
								<span className="min-w-0 flex-1 truncate">Use an existing folder</span>
							</button>
						</PopoverContent>
					</Popover>
				</div>
			</PopoverContent>
		</Popover>
	)
}
