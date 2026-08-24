/**
 * Customize tab for project and user AGENTS.md instruction files.
 */

import { Button } from "@devo/ui/components/button"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useAtomValue } from "jotai"
import { FilePlusIcon, FolderOpenIcon, RefreshCwIcon, ScrollTextIcon } from "lucide-react"
import { useMemo } from "react"
import { toast } from "sonner"
import type { RuleFileInfo } from "../../../preload/api"
import { desktopFoldersAtom } from "../../atoms/desktop-folders"
import { discoveryAtom } from "../../atoms/discovery"
import { SettingsHeader } from "../settings/settings-header"
import { SettingsRow } from "../settings/settings-row"
import { SettingsSection } from "../settings/settings-section"

const isElectron = typeof window !== "undefined" && "devo" in window

function folderName(directory: string): string {
	return directory.split(/[\\/]/).filter(Boolean).at(-1) ?? directory
}

export function RuleSettings({
	embedded = false,
	searchQuery = "",
}: {
	embedded?: boolean
	searchQuery?: string
}) {
	const desktopFolders = useAtomValue(desktopFoldersAtom)
	const discovery = useAtomValue(discoveryAtom)
	const queryClient = useQueryClient()
	const directories = useMemo(() => {
		const dirs = new Set<string>()
		for (const folder of desktopFolders) dirs.add(folder.directory)
		for (const project of discovery.projects) {
			if (project.worktree) dirs.add(project.worktree)
		}
		return [...dirs]
	}, [desktopFolders, discovery.projects])

	const { data: files = [], isLoading, error, refetch } = useQuery({
		queryKey: ["rule-files", directories],
		queryFn: async (): Promise<RuleFileInfo[]> => {
			if (!isElectron) return []
			return window.devo.rules.list(directories)
		},
		enabled: isElectron,
	})

	const create = useMutation({
		mutationFn: async (directory: string) => {
			if (!isElectron) throw new Error("Rules can only be created in the desktop app")
			return window.devo.rules.create(directory)
		},
		onSuccess: (file) => {
			void queryClient.invalidateQueries({ queryKey: ["rule-files"] })
			toast.success("Created AGENTS.md", { description: file.path })
		},
		onError: (err) => {
			toast.error("Failed to create AGENTS.md", {
				description: err instanceof Error ? err.message : String(err),
			})
		},
	})

	const openFile = async (filePath: string) => {
		if (!isElectron) return
		try {
			await window.devo.rules.open(filePath)
		} catch (err) {
			toast.error("Failed to open rule file", {
				description: err instanceof Error ? err.message : String(err),
			})
		}
	}

	const visibleFiles = files.filter((file) => {
		if (!searchQuery) return true
		const haystack = `${file.name} ${file.path} ${file.scope}`.toLowerCase()
		return haystack.includes(searchQuery.toLowerCase())
	})
	const missingProjectDirs = directories.filter((directory) => {
		if (files.some((file) => file.scope === "project" && file.directory === directory)) return false
		if (!searchQuery) return true
		return `${folderName(directory)} ${directory}`.toLowerCase().includes(searchQuery.toLowerCase())
	})

	return (
		<div className={embedded ? "space-y-6 px-8 py-8" : "space-y-10"}>
			{!embedded && (
				<SettingsHeader
					title="Rules"
					description={
						<>
							Agent instructions live in{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">AGENTS.md</code>. Devo also
							reads{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">AGENTS.override.md</code>,{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">CLAUDE.md</code>, and{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">PROMPT.md</code>.
						</>
					}
				/>
			)}

			<SettingsSection title="Instruction files">
				{isLoading && (
					<SettingsRow label="Loading" description="Looking for AGENTS.md files">
						<RefreshCwIcon className="size-4 animate-spin text-muted-foreground" />
					</SettingsRow>
				)}
				{error && (
					<SettingsRow label="Failed to load rules" description={String(error)}>
						<Button size="sm" variant="outline" onClick={() => void refetch()}>
							Retry
						</Button>
					</SettingsRow>
				)}
				{!isLoading && !error && visibleFiles.length === 0 && (
					<SettingsRow
						label={files.length === 0 ? "No instruction files yet" : "No matching files"}
						description={
							files.length === 0
								? "Create AGENTS.md in a project folder to guide the agent."
								: "Try a different search."
						}
					>
						<ScrollTextIcon className="size-4 text-muted-foreground" />
					</SettingsRow>
				)}
				{visibleFiles.map((file) => (
					<SettingsRow
						key={file.path}
						label={file.name}
						description={`${file.scope === "user" ? "User" : folderName(file.directory)} · ${file.path}`}
					>
						<Button size="sm" variant="outline" onClick={() => void openFile(file.path)}>
							<FolderOpenIcon className="size-3.5" />
							Open
						</Button>
					</SettingsRow>
				))}
			</SettingsSection>

			{missingProjectDirs.length > 0 && (
				<SettingsSection title="Create AGENTS.md">
					{missingProjectDirs.map((directory) => (
						<SettingsRow
							key={directory}
							label={folderName(directory)}
							description={directory}
						>
							<Button
								size="sm"
								variant="outline"
								disabled={create.isPending}
								onClick={() => create.mutate(directory)}
							>
								<FilePlusIcon className="size-3.5" />
								Create
							</Button>
						</SettingsRow>
					))}
				</SettingsSection>
			)}
		</div>
	)
}
