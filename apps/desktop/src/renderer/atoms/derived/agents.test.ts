import { describe, expect, test } from "bun:test"
import type { Session, SidebarProject } from "../../lib/types"
import { desktopFoldersAtom } from "../desktop-folders"
import { discoveryAtom } from "../discovery"
import { setSessionStatusAtom, upsertSessionAtom } from "../sessions"
import { appStore } from "../store"
import {
	agentFamily,
	formatRelativeTime,
	projectDisplayName,
	projectListAtom,
	projectNameFromDir,
	sortSidebarProjectsForDefaultList,
} from "./agents"

function project(
	name: string,
	directory: string,
	lastActiveAt: number,
	hasActiveAgent = false,
): SidebarProject {
	return {
		id: `${name}-id`,
		slug: `${name}-slug`,
		name,
		directory,
		agentCount: lastActiveAt > 0 ? 1 : 0,
		lastActiveAt,
		hasActiveAgent,
	}
}

describe("project list ordering", () => {
	test("default project order follows discovery order when activity fields change", () => {
		const alpha = project("alpha", "/repo/alpha", 10)
		const beta = project("beta", "/repo/beta", 90, true)
		const gamma = project("gamma", "/repo/gamma", 0)
		const activeAlpha = project("alpha", "/repo/alpha", 120, true)
		const idleBeta = project("beta", "/repo/beta", 20)
		const discoveryOrder = [beta.directory, alpha.directory, gamma.directory]

		expect(sortSidebarProjectsForDefaultList([beta, gamma, alpha], discoveryOrder)).toEqual([
			beta,
			alpha,
			gamma,
		])
		expect(sortSidebarProjectsForDefaultList([idleBeta, gamma, activeAlpha], discoveryOrder)).toEqual([
			idleBeta,
			activeAlpha,
			gamma,
		])
	})

	test("default project order falls back to name for projects outside discovery", () => {
		const alpha = project("alpha", "/repo/alpha", 10)
		const beta = project("beta", "/repo/beta", 90, true)
		const gamma = project("gamma", "/repo/gamma", 0)

		expect(sortSidebarProjectsForDefaultList([beta, gamma, alpha], [])).toEqual([
			alpha,
			beta,
			gamma,
		])
	})
})

describe("project display names", () => {
	test("derives folder names from platform-specific paths", () => {
		expect({
			windows: projectNameFromDir("C:\\Users\\lenovo\\Desktop\\devo"),
			windowsTrailingSlash: projectNameFromDir("C:\\Users\\lenovo\\Desktop\\devo\\"),
			unix: projectNameFromDir("/repo/seo_0623"),
			unixTrailingSlash: projectNameFromDir("/repo/seo_0623/"),
		}).toEqual({
			windows: "devo",
			windowsTrailingSlash: "devo",
			unix: "seo_0623",
			unixTrailingSlash: "seo_0623",
		})
	})

	test("keeps short project names and normalizes path-like API names", () => {
		expect({
			shortName: projectDisplayName("seo_0623", "C:\\Users\\lenovo\\Desktop\\seo_0623"),
			windowsPathName: projectDisplayName(
				"C:\\Users\\lenovo\\Desktop\\devo",
				"C:\\Users\\lenovo\\Desktop\\devo",
			),
			unixPathName: projectDisplayName("/repo/devo", "/repo/devo"),
			blankName: projectDisplayName("  ", "/repo/devo"),
		}).toEqual({
			shortName: "seo_0623",
			windowsPathName: "devo",
			unixPathName: "devo",
			blankName: "devo",
		})
	})
})

describe("relative session time formatting", () => {
	test("formats relative time from an explicit clock", () => {
		const now = Date.parse("2026-06-24T02:00:00.000Z")

		expect({
			now: formatRelativeTime(now - 30_000, now),
			oneMinute: formatRelativeTime(now - 60_000, now),
			minutes: formatRelativeTime(now - 42 * 60_000, now),
			hours: formatRelativeTime(now - 2 * 60 * 60_000, now),
			days: formatRelativeTime(now - 3 * 24 * 60 * 60_000, now),
		}).toEqual({
			now: "1m",
			oneMinute: "1m",
			minutes: "42m",
			hours: "2h",
			days: "3d",
		})
	})
})

describe("agent status derivation", () => {
	test("keeps idle selected sessions idle and maps active/error statuses for display", () => {
		const session: Session = {
			id: "agent-status-session",
			title: "Status session",
			directory: "/repo/status",
			time: { created: 1, updated: 1 },
		}
		appStore.set(upsertSessionAtom, { session, directory: "/repo/status" })

		const statuses = [
			[{ type: "idle" }, "idle"],
			[{ type: "busy" }, "running"],
			[{ type: "retry" }, "running"],
			[{ type: "error" }, "failed"],
		] as const

		const derived = statuses.map(([status]) => {
			appStore.set(setSessionStatusAtom, { sessionId: session.id, status })
			return appStore.get(agentFamily(session.id))?.status
		})

		expect(derived).toEqual(statuses.map(([, expected]) => expected))
	})
})

describe("sidebar project list", () => {
	test("includes discovered worktrees even without desktop folders", () => {
		appStore.set(desktopFoldersAtom, [])
		appStore.set(discoveryAtom, {
			loaded: true,
			loading: false,
			error: null,
			phase: "ready",
			projects: [
				{
					id: "alpha-id",
					name: "alpha",
					worktree: "/repo/alpha",
					path: { root: "/repo/alpha" },
					time: { created: 1, updated: 1 },
					sandboxes: [],
				},
				{
					id: "beta-id",
					name: "beta",
					worktree: "/repo/beta",
					path: { root: "/repo/beta" },
					time: { created: 1, updated: 1 },
					sandboxes: [],
				},
			],
		})

		const directories = appStore.get(projectListAtom).map((item) => item.directory)
		expect(directories).toContain("/repo/alpha")
		expect(directories).toContain("/repo/beta")
	})

	test("drops a directory from the sidebar when it leaves both desktop folders and discovery", () => {
		const directory = "/repo/remove-folder"
		appStore.set(desktopFoldersAtom, [
			{ id: "desktop-folder-remove", directory, name: "remove-folder", addedAt: 1 },
		])
		appStore.set(discoveryAtom, {
			loaded: true,
			loading: false,
			error: null,
			phase: "ready",
			projects: [
				{
					id: "remove-id",
					name: "remove-folder",
					worktree: directory,
					path: { root: directory },
					time: { created: 1, updated: 1 },
					sandboxes: [],
				},
			],
		})

		expect(appStore.get(projectListAtom).map((item) => item.directory)).toContain(directory)

		appStore.set(desktopFoldersAtom, [])
		expect(appStore.get(projectListAtom).map((item) => item.directory)).toContain(directory)

		appStore.set(discoveryAtom, {
			loaded: true,
			loading: false,
			error: null,
			phase: "ready",
			projects: [],
		})
		expect(appStore.get(projectListAtom).map((item) => item.directory)).not.toContain(directory)
	})
})
