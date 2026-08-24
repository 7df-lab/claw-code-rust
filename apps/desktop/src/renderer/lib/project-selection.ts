import { directoriesMatch } from "./directory-path"
import type { SidebarProject } from "./types"

export function isFilesystemRootDirectory(directory: string): boolean {
	const normalized = directory.replace(/\\/g, "/").replace(/\/+$/, "")
	return normalized === "" || normalized === "/" || /^[a-zA-Z]:$/.test(normalized)
}

function isAvailableProject(
	project: SidebarProject,
	unavailableDirectories: ReadonlySet<string> | undefined,
): boolean {
	if (unavailableDirectories?.has(project.directory)) return false
	if (project.folderStatus && project.folderStatus !== "available") return false
	return true
}

function defaultProjectDirectory(
	projects: SidebarProject[],
	options: {
		unavailableDirectories?: ReadonlySet<string>
		lastUsedDirectory?: string | null
	},
): string {
	const available = projects.filter((project) => isAvailableProject(project, options.unavailableDirectories))
	const pool = (available.length > 0 ? available : projects).filter(
		(project) => !isFilesystemRootDirectory(project.directory) || projects.length === 1,
	)
	const candidates = pool.length > 0 ? pool : projects
	if (candidates.length === 0) return ""

	if (options.lastUsedDirectory) {
		const lastUsed = candidates.find((project) =>
			directoriesMatch(project.directory, options.lastUsedDirectory!),
		)
		if (lastUsed) return lastUsed.directory
	}

	const ranked = [...candidates].sort((left, right) => {
		if (left.lastActiveAt !== right.lastActiveAt) return right.lastActiveAt - left.lastActiveAt
		if (left.hasActiveAgent !== right.hasActiveAgent) return Number(right.hasActiveAgent) - Number(left.hasActiveAgent)
		return left.name.localeCompare(right.name)
	})
	return ranked[0]?.directory ?? ""
}

/**
 * Resolves the project directory NewChat should target.
 *
 * Route context wins first. Without route context, keep the user's current
 * explicit selection if it still exists, then fall back to last-used or the
 * most recently active available project so the composer always has a cwd.
 */
export function resolveSelectedProjectDirectory(
	projects: SidebarProject[],
	projectSlug: string | undefined,
	currentDirectory: string,
	options: {
		preserveCurrentDirectory?: boolean
		unavailableDirectories?: ReadonlySet<string>
		lastUsedDirectory?: string | null
	} = {},
): string {
	if (projects.length === 0) return currentDirectory

	if (projectSlug) {
		const routeProject = projects.find((project) => project.slug === projectSlug)
		if (routeProject) return routeProject.directory
	}

	if (
		options.preserveCurrentDirectory &&
		currentDirectory &&
		!options.unavailableDirectories?.has(currentDirectory) &&
		projects.some((project) => project.directory === currentDirectory)
	) {
		return currentDirectory
	}

	return defaultProjectDirectory(projects, options)
}

export function resolveNewChatRoute(
	projects: SidebarProject[],
	currentProjectSlug: string | undefined,
	lastUsedDirectory: string | null,
): { to: "/" } | { to: "/project/$projectSlug"; params: { projectSlug: string } } {
	if (currentProjectSlug) {
		const current = projects.find((project) => project.slug === currentProjectSlug)
		if (current) {
			return { to: "/project/$projectSlug", params: { projectSlug: current.slug } }
		}
	}

	const directory = resolveSelectedProjectDirectory(projects, undefined, "", {
		lastUsedDirectory,
	})
	const project = projects.find((project) => project.directory === directory)
	if (project) {
		return { to: "/project/$projectSlug", params: { projectSlug: project.slug } }
	}
	return { to: "/" }
}

export function navigateToNewChat(
	navigate: (opts: { to: string; params?: Record<string, string> }) => unknown,
	projects: SidebarProject[],
	currentProjectSlug: string | undefined,
	lastUsedDirectory: string | null,
): void {
	const route = resolveNewChatRoute(projects, currentProjectSlug, lastUsedDirectory)
	if (route.to === "/project/$projectSlug") {
		navigate({ to: "/project/$projectSlug", params: route.params })
		return
	}
	navigate({ to: "/" })
}
