import { atom } from "jotai"
import { atomFamily } from "jotai-family"
import type {
	Agent,
	AgentStatus,
	DevoProject,
	SessionStatus,
	SidebarProject,
} from "../../lib/types"
import type { DesktopFolder } from "../../../preload/api"
import { desktopFolderStatusByDirectoryAtom, desktopFoldersAtom } from "../desktop-folders"
import { discoveryAtom } from "../discovery"
import { sessionFamily, sessionIdsAtom } from "../sessions"
import { effectivePermissionFamily, effectiveQuestionFamily } from "./session-requests"
import { directoriesMatch, normalizeDirectoryPath } from "../../lib/directory-path"

// ============================================================
// Structural equality for Agent objects
// ============================================================

/**
 * Shallow-compare two Agent objects by their identity and UI-relevant fields.
 * Arrays like `permissions` and `questions` are compared by length + first-element
 * identity, which is sufficient since they come from the same atom and are replaced
 * wholesale on updates.
 */
function agentEqual(prev: Agent | null, next: Agent | null): boolean {
	if (prev === next) return true
	if (!prev || !next) return false
	return (
		prev.id === next.id &&
		prev.name === next.name &&
		prev.status === next.status &&
		prev.project === next.project &&
		prev.projectSlug === next.projectSlug &&
		prev.directory === next.directory &&
		prev.projectDirectory === next.projectDirectory &&
		prev.branch === next.branch &&
		prev.duration === next.duration &&
		// currentActivity is derived from tree-scoped requests; always compare it
		// so status changes from descendant sub-agents propagate to the sidebar.
		prev.currentActivity === next.currentActivity &&
		prev.parentId === next.parentId &&
		prev.forkFromId === next.forkFromId &&
		prev.atTurnId === next.atTurnId &&
		prev.worktreePath === next.worktreePath &&
		prev.worktreeBranch === next.worktreeBranch &&
		prev.createdAt === next.createdAt &&
		prev.lastActiveAt === next.lastActiveAt &&
		prev.hasUnreadCompletion === next.hasUnreadCompletion &&
		prev.titleGenerating === next.titleGenerating &&
		prev.permissions.length === next.permissions.length &&
		prev.questions.length === next.questions.length &&
		prev.permissions[0] === next.permissions[0] &&
		prev.questions[0] === next.questions[0]
	)
}

// ============================================================
// Helpers (moved from hooks/use-agents.ts)
// ============================================================

function deriveAgentStatus(
	status: SessionStatus,
	hasPermissions: boolean,
	hasQuestions: boolean,
): AgentStatus {
	if (hasPermissions || hasQuestions) return "waiting"
	switch (status.type) {
		case "busy":
			return "running"
		case "retry":
			return "running"
		case "error":
			return "failed"
		case "failed":
			return "failed"
		case "idle":
			return "idle"
		default:
			return "idle"
	}
}

export function formatRelativeTime(timestampMs: number, nowMs = Date.now()): string {
	const seconds = Math.max(0, Math.floor((nowMs - timestampMs) / 1000))
	if (seconds < 60) return "1m"
	const minutes = Math.floor(seconds / 60)
	if (minutes < 60) return `${minutes}m`
	const hours = Math.floor(minutes / 60)
	if (hours < 24) return `${hours}h`
	const days = Math.floor(hours / 24)
	if (days < 30) return `${days}d`
	const months = Math.floor(days / 30)
	return `${months}mo`
}

export function formatElapsed(startMs: number): string {
	const seconds = Math.max(0, Math.floor((Date.now() - startMs) / 1000))
	if (seconds < 60) return `${seconds}s`
	const minutes = Math.floor(seconds / 60)
	const remainingSeconds = seconds % 60
	if (minutes < 60) return `${minutes}m ${remainingSeconds}s`
	const hours = Math.floor(minutes / 60)
	const remainingMinutes = minutes % 60
	return `${hours}h ${remainingMinutes}m`
}

export function projectNameFromDir(directory: string): string {
	const trimmed = directory.replace(/[\\/]+$/, "")
	if (!trimmed) return "/"
	return trimmed.split(/[\\/]/).filter(Boolean).at(-1) ?? trimmed
}

function isPathLikeProjectName(name: string): boolean {
	return /[\\/]/.test(name)
}

export function projectDisplayName(name: string | null | undefined, directory: string): string {
	const trimmed = name?.trim()
	if (!trimmed || isPathLikeProjectName(trimmed)) return projectNameFromDir(directory)
	return trimmed
}

// ============================================================
// Project slug system
// ============================================================

interface ProjectEntry {
	id: string
	name: string
	directory: string
}

function buildProjectSlugMap(
	projects: ProjectEntry[],
): Map<string, { id: string; name: string; slug: string }> {
	const byDir = new Map<string, ProjectEntry>()
	for (const p of projects) {
		const existing = byDir.get(p.directory)
		if (!existing || (existing.id.startsWith("dir-") && !p.id.startsWith("dir-"))) {
			byDir.set(p.directory, p)
		}
	}

	const result = new Map<string, { id: string; name: string; slug: string }>()
	for (const entry of byDir.values()) {
		const slug = `${entry.name}-${entry.id.slice(0, 12)}`
		result.set(entry.directory, { id: entry.id, name: entry.name, slug })
	}
	return result
}

export function sortSidebarProjectsForDefaultList(
	projects: SidebarProject[],
	discoveryOrder: readonly string[],
): SidebarProject[] {
	const orderByDirectory = new Map<string, number>()
	for (let i = 0; i < discoveryOrder.length; i++) {
		orderByDirectory.set(discoveryOrder[i], i)
	}

	return [...projects].sort((a, b) => {
		const orderA = orderByDirectory.get(a.directory)
		const orderB = orderByDirectory.get(b.directory)
		if (orderA !== undefined && orderB !== undefined && orderA !== orderB) {
			return orderA - orderB
		}
		if (orderA !== undefined) return -1
		if (orderB !== undefined) return 1

		const nameDiff = a.name.localeCompare(b.name)
		if (nameDiff !== 0) return nameDiff
		return a.directory.localeCompare(b.directory)
	})
}

// ============================================================
// Sandbox (worktree) directory mapping
// ============================================================

/**
 * Builds a set of all sandbox directories across all discovered projects.
 * A "sandbox" is a worktree directory that belongs to a parent project.
 * These should not appear as top-level projects in the sidebar.
 */
function buildSandboxDirSet(projects: DevoProject[]): Set<string> {
	const sandboxDirs = new Set<string>()
	for (const project of projects) {
		if (project.sandboxes) {
			for (const dir of project.sandboxes) {
				sandboxDirs.add(dir)
			}
		}
	}
	return sandboxDirs
}

/**
 * Builds a map from sandbox directory -> parent project worktree directory.
 * Used to remap sessions running in a sandbox back to their parent project.
 */
function buildSandboxToParentMap(projects: DevoProject[]): Map<string, string> {
	const map = new Map<string, string>()
	for (const project of projects) {
		if (!project.worktree || !project.sandboxes) continue
		for (const dir of project.sandboxes) {
			map.set(dir, project.worktree)
		}
	}
	return map
}

/**
 * Builds a map from parent project directory -> set of its sandbox directories.
 * Used by projectSessionIdsFamily to include sandbox sessions under the parent.
 */
function buildParentToSandboxesMap(projects: DevoProject[]): Map<string, Set<string>> {
	const map = new Map<string, Set<string>>()
	for (const project of projects) {
		if (!project.worktree || !project.sandboxes?.length) continue
		map.set(project.worktree, new Set(project.sandboxes))
	}
	return map
}

function collectAllProjects(
	liveSessionDirs: Map<string, string>,
	desktopFolders: readonly DesktopFolder[],
	discovery: {
		loaded: boolean
		projects: DevoProject[]
	},
): ProjectEntry[] {
	const entries: ProjectEntry[] = []
	const seenDirs = new Set<string>()

	for (const folder of desktopFolders) {
		if (!folder.directory) continue
		const seenKey = normalizeDirectoryPath(folder.directory)
		if (seenDirs.has(seenKey)) continue
		seenDirs.add(seenKey)
		entries.push({
			id: folder.id,
			name: projectDisplayName(folder.name, folder.directory),
			directory: folder.directory,
		})
	}

	// Build sandbox set to filter out worktree projects
	const sandboxDirs = discovery.loaded ? buildSandboxDirSet(discovery.projects) : new Set<string>()

	// Discovery projects (from API), excluding sandboxes
	if (discovery.loaded) {
		for (const project of discovery.projects) {
			if (!project.worktree) continue
			const seenKey = normalizeDirectoryPath(project.worktree)
			if (seenDirs.has(seenKey)) continue
			if ([...sandboxDirs].some((dir) => directoriesMatch(dir, project.worktree))) continue
			seenDirs.add(seenKey)
			entries.push({
				id: project.id,
				name: projectDisplayName(project.name, project.worktree),
				directory: project.worktree,
			})
		}
	}

	// Live session directories (may include directories not in any project).
	// Skip directories that are sandboxes of a known project.
	for (const [, directory] of liveSessionDirs) {
		if (!directory) continue
		const seenKey = normalizeDirectoryPath(directory)
		if (seenDirs.has(seenKey)) continue
		if ([...sandboxDirs].some((dir) => directoriesMatch(dir, directory))) continue
		seenDirs.add(seenKey)
		let hash = 0
		for (let i = 0; i < directory.length; i++) {
			hash = (hash * 31 + directory.charCodeAt(i)) | 0
		}
		entries.push({
			id: `dir-${Math.abs(hash).toString(16).padStart(8, "0")}`,
			name: projectDisplayName(undefined, directory),
			directory,
		})
	}

	return entries
}

// ============================================================
// Derived atoms: sandbox mappings + project slug map
// ============================================================

/**
 * Derived atom that computes sandbox directory mappings from discovery data.
 * Only recomputes when discovery changes (loaded once per connection).
 *
 * - `sandboxToParent`: maps sandbox dir -> parent project dir
 * - `parentToSandboxes`: maps parent project dir -> set of sandbox dirs
 *
 * Used by projectSessionIdsFamily (to absorb sandbox sessions into parent)
 * and agentFamily (to remap project name/slug for sandbox sessions).
 */
export const sandboxMappingsAtom = atom((get) => {
	const discovery = get(discoveryAtom)
	if (!discovery.loaded) {
		return {
			sandboxToParent: new Map<string, string>(),
			parentToSandboxes: new Map<string, Set<string>>(),
		}
	}
	return {
		sandboxToParent: buildSandboxToParentMap(discovery.projects),
		parentToSandboxes: buildParentToSandboxesMap(discovery.projects),
	}
})

/**
 * Lightweight derived atom that maps directory -> { id, slug }.
 * Only depends on session directories (stable after creation) and discovery
 * (loaded once). This avoids recomputing slugs when session status/permissions change.
 */
const projectSlugMapAtom = atom((get) => {
	const sessionIds = get(sessionIdsAtom)
	const discovery = get(discoveryAtom)
	const desktopFolders = get(desktopFoldersAtom)

	const liveSessionDirs = new Map<string, string>()
	for (const id of sessionIds) {
		const entry = get(sessionFamily(id))
		if (!entry) continue
		liveSessionDirs.set(id, entry.directory)
	}

	const allProjects = collectAllProjects(liveSessionDirs, desktopFolders, discovery)
	return buildProjectSlugMap(allProjects)
})

// ============================================================
// Per-session agent selector (reads ONE sessionFamily atom)
// ============================================================

/**
 * Derives a full `Agent` for a single session. Only subscribes to that session's
 * `sessionFamily` atom + the shared `projectSlugMapAtom`, so status/permission
 * changes on OTHER sessions do not trigger re-derivation.
 */
export const agentFamily = atomFamily((sessionId: string) => {
	let prev: Agent | null = null
	return atom((get) => {
		const entry = get(sessionFamily(sessionId))
		if (!entry) {
			prev = null
			return null
		}

		const slugMap = get(projectSlugMapAtom)
		const { sandboxToParent } = get(sandboxMappingsAtom)
		const { session, status, directory } = entry

		// Use tree-scoped requests to determine blocking status so the parent
		// session shows "waiting" when any descendant sub-agent has a pending
		// permission or question — not just its own.
		const hasTreePermission = get(effectivePermissionFamily(session.id)) !== undefined
		const hasTreeQuestion = get(effectiveQuestionFamily(session.id)) !== undefined
		const agentStatus = deriveAgentStatus(status, hasTreePermission, hasTreeQuestion)

		const { permissions, questions } = entry
		const created = session.time.created
		const lastActiveAt = session.time.lastActivity ?? session.time.updated ?? session.time.created

		// If this session's directory is a sandbox (worktree), resolve the parent
		// project directory for name/slug display so it groups visually under the parent.
		const parentDir = sandboxToParent.get(directory)
		const displayDir = parentDir ?? directory
		const projectInfo = slugMap.get(displayDir)
		const projectName = projectInfo?.name ?? projectNameFromDir(displayDir)

		// Derive currentActivity from tree-scoped requests first, then own status.
		// This ensures "waiting for approval" shows even when the permission is from a sub-agent.
		const effectivePerm = get(effectivePermissionFamily(session.id))
		const effectiveQ = get(effectiveQuestionFamily(session.id))

		const next: Agent = {
			id: session.id,
			sessionId: session.id,
			name: session.title || "New Chat",
			titleGenerating: false,
			status: agentStatus,
			environment: "local" as const,
			project: projectName,
			projectSlug: projectInfo?.slug ?? projectName,
			directory,
			projectDirectory: displayDir,
			branch: entry.branch ?? "",
			duration: formatRelativeTime(lastActiveAt),
			currentActivity: effectiveQ
				? `Asking: ${effectiveQ.request.questions[0]?.header ?? "Question"}`
				: effectivePerm
					? `Waiting for approval: ${effectivePerm.request.permission}`
					: status.type === "busy" || status.type === "retry"
						? "Working..."
						: undefined,
			activities: [],
			permissions,
			questions,
			parentId: session.parentID,
			forkFromId: session.forkFromId,
			atTurnId: session.atTurnId,
			worktreePath: entry.worktreePath,
			worktreeBranch: entry.worktreeBranch,
			createdAt: created,
			lastActiveAt,
			hasUnreadCompletion: entry.hasUnreadCompletion ?? false,
		}

		// Return the previous reference if structurally equal to avoid
		// downstream memo() invalidation in SessionItem and friends.
		if (agentEqual(prev, next)) return prev!
		prev = next
		return next
	})
})

/**
 * Reads just the session title for a given session ID.
 * Used for breadcrumb "parent session name" lookups without subscribing
 * to the full agents list.
 */
export const sessionNameFamily = atomFamily((sessionId: string) =>
	atom((get) => {
		const entry = get(sessionFamily(sessionId))
		if (!entry) return undefined
		return entry.session.title || "New Chat"
	}),
)

// ============================================================
// Derived atom: agents list
// ============================================================

/**
 * All agents derived from live sessions.
 * With API-first discovery, there are no more "offline-only" discovered sessions
 * since sessions are loaded directly from the API into the session atom family.
 *
 * Uses structural equality on the array elements so downstream subscribers
 * (SidebarLayout, CommandPalette) don't re-render when individual agent
 * references are stable.
 */
export const agentsAtom = (() => {
	let prevAgents: Agent[] = []
	return atom((get) => {
		const sessionIds = get(sessionIdsAtom)
		const agents: Agent[] = []

		for (const id of sessionIds) {
			const agent = get(agentFamily(id))
			if (agent) agents.push(agent)
		}

		// Return the previous array if every element is referentially identical.
		// This is cheap because agentFamily already stabilizes references.
		if (agents.length === prevAgents.length && agents.every((a, i) => a === prevAgents[i])) {
			return prevAgents
		}
		prevAgents = agents
		return agents
	})
})()

// ============================================================
// Per-project session IDs for granular sidebar subscriptions
// ============================================================

/**
 * Returns the list of session IDs belonging to a specific project directory.
 * Keyed by directory path. Each ProjectFolder subscribes to its own family
 * member, so adding/removing sessions in project A does not re-render project B.
 *
 * Also includes sessions running in sandbox (worktree) directories that belong
 * to this project, so worktree sessions appear under the parent project.
 *
 * Uses structural equality on the array to avoid unnecessary re-renders
 * when the same set of IDs is returned.
 */
export const projectSessionIdsFamily = atomFamily((directory: string) => {
	let prev: string[] = []
	return atom((get) => {
		const sessionIds = get(sessionIdsAtom)
		const { parentToSandboxes } = get(sandboxMappingsAtom)

		// Directories that belong to this project: the project dir itself + its sandboxes
		const sandboxes = parentToSandboxes.get(directory)

		const ids: string[] = []
		for (const id of sessionIds) {
			const entry = get(sessionFamily(id))
			if (!entry) continue
			// Hide sub-agent sessions in the sidebar (they still exist in the store
			// for message/part lookups and direct navigation)
			if (entry.session.parentID) continue
			// Match the project directory itself, or any of its sandbox directories
			if (
				!directoriesMatch(entry.directory, directory) &&
				![...sandboxes ?? []].some((sandbox) => directoriesMatch(sandbox, entry.directory))
			) {
				continue
			}
			ids.push(id)
		}
		// Structural equality: return previous array if contents are the same
		if (ids.length === prev.length && ids.every((id, i) => id === prev[i])) {
			return prev
		}
		prev = ids
		return ids
	})
})

// ============================================================
// Derived atom: project list for sidebar
// ============================================================

export const projectListAtom = (() => {
	let prevProjects: SidebarProject[] = []

	function projectListEqual(a: SidebarProject[], b: SidebarProject[]): boolean {
		if (a.length !== b.length) return false
		for (let i = 0; i < a.length; i++) {
			const pa = a[i]
			const pb = b[i]
			if (
				pa.id !== pb.id ||
				pa.slug !== pb.slug ||
				pa.name !== pb.name ||
				pa.directory !== pb.directory ||
				pa.agentCount !== pb.agentCount ||
				pa.lastActiveAt !== pb.lastActiveAt ||
				pa.hasActiveAgent !== pb.hasActiveAgent ||
				pa.folderStatus !== pb.folderStatus
			) {
				return false
			}
		}
		return true
	}

	return atom((get) => {
		const sessionIds = get(sessionIdsAtom)
		const desktopFolders = get(desktopFoldersAtom)
		const folderStatuses = get(desktopFolderStatusByDirectoryAtom)
		const discovery = get(discoveryAtom)
		const slugMap = get(projectSlugMapAtom)
		const { sandboxToParent } = get(sandboxMappingsAtom)

		const liveSessionDirs = new Map<string, string>()
		for (const id of sessionIds) {
			const entry = get(sessionFamily(id))
			if (!entry) continue
			liveSessionDirs.set(id, entry.directory)
		}

		const projects = new Map<string, SidebarProject>()
		for (const entry of collectAllProjects(liveSessionDirs, desktopFolders, discovery)) {
			const projectInfo = slugMap.get(entry.directory)
			const name = projectDisplayName(projectInfo?.name ?? entry.name, entry.directory)
			projects.set(entry.directory, {
				id: projectInfo?.id ?? entry.id,
				slug: projectInfo?.slug ?? `${name}-${entry.id.slice(0, 12)}`,
				name,
				directory: entry.directory,
				agentCount: 0,
				lastActiveAt: 0,
				hasActiveAgent: false,
				folderStatus: folderStatuses[entry.directory],
			})
		}

		for (const id of sessionIds) {
			const entry = get(sessionFamily(id))
			if (!entry) continue
			if (entry.session.parentID) continue
			if (!entry.directory) continue

			const parentDir = sandboxToParent.get(entry.directory)
			const dir = parentDir ?? entry.directory
			let existing = projects.get(dir)
			if (!existing) {
				for (const project of projects.values()) {
					if (directoriesMatch(project.directory, dir)) {
						existing = project
						break
					}
				}
			}
			if (!existing) continue
			const sessionTime =
				entry.session.time.lastActivity ??
				entry.session.time.updated ??
				entry.session.time.created ??
				0
			const isActive =
				entry.status.type === "busy" ||
				entry.permissions.length > 0 ||
				entry.questions.length > 0

			existing.agentCount += 1
			if (sessionTime > existing.lastActiveAt) existing.lastActiveAt = sessionTime
			if (isActive) existing.hasActiveAgent = true
		}

		const discoveryOrder = [
			...desktopFolders.map((folder) => folder.directory),
			...discovery.projects.map((project) => project.worktree).filter((directory): directory is string => !!directory),
		]
		const next = sortSidebarProjectsForDefaultList([...projects.values()], discoveryOrder)

		if (projectListEqual(prevProjects, next)) return prevProjects
		prevProjects = next
		return next
	})
})()
