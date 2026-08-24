import { directoriesMatch } from "../../lib/directory-path"
import type { Agent, SidebarProject } from "../../lib/types"

export type SidebarSort = "updated" | "created"

export interface SidebarPreferences {
	sort: SidebarSort
}

export const DEFAULT_SIDEBAR_PREFERENCES: SidebarPreferences = {
	sort: "updated",
}

export interface SidebarProjectDisplayItem {
	type: "project"
	project: SidebarProject
	sessions: Agent[]
}

export type SidebarDisplayItem = SidebarProjectDisplayItem

export interface BuildSidebarItemsArgs {
	projects: SidebarProject[]
	agents: Agent[]
	projectSessionsByDirectory: Map<string, Agent[]>
	preferences: SidebarPreferences
	projectOrder?: ReadonlyMap<string, number>
}

export function groupAgentsByProject(
	agents: Agent[],
	projects: SidebarProject[],
): Map<string, Agent[]> {
	const grouped = new Map<string, Agent[]>()
	for (const project of projects) {
		grouped.set(project.directory, [])
	}
	for (const agent of agents) {
		if (agent.parentId) continue
		const sessionDir = agent.projectDirectory || agent.directory
		const project = projects.find((item) => directoriesMatch(item.directory, sessionDir))
		if (!project) continue
		const existing = grouped.get(project.directory)
		if (existing) existing.push(agent)
		else grouped.set(project.directory, [agent])
	}
	return grouped
}

function sortSessions(sessions: Agent[], sort: SidebarSort): Agent[] {
	const sorted = [...sessions]
	sorted.sort((a, b) => {
		const aTime = sort === "created" ? a.createdAt : a.lastActiveAt
		const bTime = sort === "created" ? b.createdAt : b.lastActiveAt
		const timeDiff = bTime - aTime
		if (timeDiff !== 0) return timeDiff
		return a.name.localeCompare(b.name)
	})
	return sorted
}

function sortProjectsByStableOrder(
	projects: SidebarProject[],
	projectOrder: ReadonlyMap<string, number> | undefined,
): SidebarProject[] {
	if (!projectOrder) return projects

	const sorted = [...projects]
	sorted.sort((a, b) => {
		const orderA = projectOrder.get(a.directory) ?? Number.MAX_SAFE_INTEGER
		const orderB = projectOrder.get(b.directory) ?? Number.MAX_SAFE_INTEGER
		const orderDiff = orderA - orderB
		if (orderDiff !== 0) return orderDiff

		const nameDiff = a.name.localeCompare(b.name)
		if (nameDiff !== 0) return nameDiff
		return a.directory.localeCompare(b.directory)
	})
	return sorted
}

export function buildSidebarItems({
	projects,
	projectSessionsByDirectory,
	preferences,
	projectOrder,
}: BuildSidebarItemsArgs): SidebarDisplayItem[] {
	const projectRows = sortProjectsByStableOrder(projects, projectOrder)

	return projectRows.map((project) => ({
		type: "project",
		project,
		sessions: sortSessions(projectSessionsByDirectory.get(project.directory) ?? [], preferences.sort),
	}))
}
