/**
 * Settings tab for discovered skills and per-skill enablement.
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import { Switch } from "@devo/ui/components/switch"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useAtomValue } from "jotai"
import { BookOpenIcon, RefreshCwIcon } from "lucide-react"
import { serverConnectedAtom } from "../../atoms/connection"
import { discoveryAtom } from "../../atoms/discovery"
import { getBaseClient, getProjectClient } from "../../services/connection-manager"
import { SettingsHeader } from "./settings-header"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

interface SkillRow {
	id: string
	name: string
	description: string
	path: string
	enabled: boolean
	source: string
}

function skillSourceLabel(source: unknown): string {
	if (typeof source === "string") return source
	if (source && typeof source === "object") {
		const value = source as Record<string, unknown>
		if ("User" in value || source === "User") return "user"
		if (typeof value.Workspace === "object" || "cwd" in value) return "workspace"
		if (typeof value.Plugin === "object" || "plugin_id" in value || "pluginId" in value) {
			return "plugin"
		}
		if ("System" in value) return "system"
		if ("Admin" in value) return "admin"
	}
	return "unknown"
}

function useSkillsClient() {
	const connected = useAtomValue(serverConnectedAtom)
	const discovery = useAtomValue(discoveryAtom)
	const directory = discovery.projects[0]?.worktree ?? null
	const client = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
	return { client, connected, directory }
}

export function SkillSettings({
	embedded = false,
	searchQuery = "",
}: {
	embedded?: boolean
	searchQuery?: string
}) {
	const { client, connected, directory } = useSkillsClient()
	const queryClient = useQueryClient()

	const { data: skills = [], isLoading, error, refetch } = useQuery({
		queryKey: ["skills-settings", directory],
		queryFn: async (): Promise<SkillRow[]> => {
			if (!client) return []
			const result = await client.app.skills()
			return ((result.data ?? []) as Array<Record<string, unknown>>).map((skill) => ({
				id: String(skill.id ?? skill.path ?? skill.name ?? ""),
				name: String(skill.name ?? ""),
				description: String(skill.shortDescription ?? skill.short_description ?? skill.description ?? ""),
				path: String(skill.path ?? ""),
				enabled: skill.enabled !== false,
				source: skillSourceLabel(skill.source),
			}))
		},
		enabled: connected && !!client,
	})

	const toggle = useMutation({
		mutationFn: async (params: { path: string; enabled: boolean }) => {
			if (!client?.app.setSkillEnabled) throw new Error("Not connected")
			await client.app.setSkillEnabled(params)
		},
		onSuccess: () => {
			void queryClient.invalidateQueries({ queryKey: ["skills-settings"] })
			void queryClient.invalidateQueries({ queryKey: ["skills"] })
		},
	})

	const visibleSkills = skills.filter((skill) => {
		if (!searchQuery) return true
		const haystack = `${skill.name} ${skill.description} ${skill.path} ${skill.source}`.toLowerCase()
		return haystack.includes(searchQuery.toLowerCase())
	})

	return (
		<div className={embedded ? "space-y-6 px-8 py-8" : "space-y-10"}>
			{!embedded && (
				<SettingsHeader
					title="Skills"
					description={
						<>
							Skills are discovered from user, workspace, plugin, and system roots. Disable a skill to
							keep it off this machine. Insert one in chat with{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">/skills</code> or{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">@</code>.
						</>
					}
				/>
			)}

			<SettingsSection title="Installed skills">
				{isLoading && (
					<SettingsRow label="Loading" description="Fetching discovered skills">
						<RefreshCwIcon className="size-4 animate-spin text-muted-foreground" />
					</SettingsRow>
				)}
				{error && (
					<SettingsRow label="Failed to load skills" description={String(error)}>
						<Button size="sm" variant="outline" onClick={() => void refetch()}>
							Retry
						</Button>
					</SettingsRow>
				)}
				{!isLoading && !error && visibleSkills.length === 0 && (
					<SettingsRow
						label={skills.length === 0 ? "No skills discovered" : "No matching skills"}
						description={
							skills.length === 0
								? "Add a SKILL.md under a skills/ directory, then reopen the session."
								: "Try a different search."
						}
					>
						<BookOpenIcon className="size-4 text-muted-foreground" />
					</SettingsRow>
				)}
				{visibleSkills.map((skill) => (
					<SettingsRow
						key={skill.path || skill.id}
						label={skill.name}
						description={skill.description || skill.path}
					>
						<div className="flex items-center gap-2">
							<Badge variant="secondary">{skill.source}</Badge>
							<Switch
								checked={skill.enabled}
								disabled={!skill.path || toggle.isPending}
								onCheckedChange={(checked) =>
									toggle.mutate({ path: skill.path, enabled: checked === true })
								}
							/>
						</div>
					</SettingsRow>
				))}
			</SettingsSection>
		</div>
	)
}
