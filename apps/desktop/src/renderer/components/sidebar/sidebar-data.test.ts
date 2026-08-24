import { describe, expect, test } from "bun:test"
import type { Agent, SidebarProject } from "../../lib/types"
import { buildSidebarItems, groupAgentsByProject, type SidebarPreferences } from "./sidebar-data"

function project(name: string, directory: string, lastActiveAt: number): SidebarProject {
	return {
		id: `${name}-id`,
		slug: `${name}-slug`,
		name,
		directory,
		agentCount: 2,
		lastActiveAt,
		hasActiveAgent: false,
	}
}

function agent(
	id: string,
	projectInfo: SidebarProject,
	createdAt: number,
	lastActiveAt: number,
): Agent {
	return {
		id,
		sessionId: id,
		name: `${id} session`,
		status: "idle",
		environment: "local",
		project: projectInfo.name,
		projectSlug: projectInfo.slug,
		directory: projectInfo.directory,
		projectDirectory: projectInfo.directory,
		branch: "main",
		duration: "1h",
		activities: [],
		permissions: [],
		questions: [],
		createdAt,
		lastActiveAt,
	}
}

describe("sidebar data helpers", () => {
	const alpha = project("alpha", "/repo/alpha", 200)
	const beta = project("beta", "/repo/beta", 400)
	const alphaOlder = agent("alpha-older", alpha, 10, 30)
	const alphaNewer = agent("alpha-newer", alpha, 20, 80)
	const betaOnly = agent("beta-only", beta, 30, 60)

	test("by-project groups preserve project sections and loaded sessions", () => {
		const preferences: SidebarPreferences = { sort: "updated" }

		expect(
			buildSidebarItems({
				projects: [alpha, beta],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map([
					[alpha.directory, [alphaOlder, alphaNewer]],
					[beta.directory, [betaOnly]],
				]),
				preferences,
			}),
		).toEqual([
			{ type: "project", project: alpha, sessions: [alphaNewer, alphaOlder] },
			{ type: "project", project: beta, sessions: [betaOnly] },
		])
	})

	test("by-project order can remain stable when project activity changes", () => {
		const preferences: SidebarPreferences = { sort: "updated" }
		const stableOrder = new Map([
			[alpha.directory, 0],
			[beta.directory, 1],
		])

		expect(
			buildSidebarItems({
				projects: [beta, alpha],
				agents: [alphaOlder, betaOnly],
				projectSessionsByDirectory: new Map([
					[alpha.directory, [alphaOlder]],
					[beta.directory, [betaOnly]],
				]),
				preferences,
				projectOrder: stableOrder,
			}),
		).toEqual([
			{ type: "project", project: alpha, sessions: [alphaOlder] },
			{ type: "project", project: beta, sessions: [betaOnly] },
		])
	})

	test("empty stored folders produce an empty sidebar even when sessions exist", () => {
		const preferences: SidebarPreferences = { sort: "updated" }

		expect(
			buildSidebarItems({
				projects: [],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map([
					[alpha.directory, [alphaOlder, alphaNewer]],
					[beta.directory, [betaOnly]],
				]),
				preferences,
			}),
		).toEqual([])
	})

	test("created sort orders sessions by creation time instead of update time", () => {
		const preferences: SidebarPreferences = { sort: "created" }

		expect(
			buildSidebarItems({
				projects: [alpha],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map([[alpha.directory, [alphaOlder, alphaNewer]]]),
				preferences,
			}),
		).toEqual([{ type: "project", project: alpha, sessions: [alphaNewer, alphaOlder] }])
	})

	test("unstored session directories are hidden from folder tree", () => {
		const preferences: SidebarPreferences = { sort: "updated" }

		expect(
			buildSidebarItems({
				projects: [alpha],
				agents: [alphaOlder, alphaNewer, betaOnly],
				projectSessionsByDirectory: new Map([
					[alpha.directory, [alphaOlder, alphaNewer]],
					[beta.directory, [betaOnly]],
				]),
				preferences,
			}),
		).toEqual([{ type: "project", project: alpha, sessions: [alphaNewer, alphaOlder] }])
	})

	test("missing folder rows remain present with their matching sessions", () => {
		const preferences: SidebarPreferences = { sort: "updated" }
		const missingAlpha = { ...alpha, folderStatus: "missing" as const }

		expect(
			buildSidebarItems({
				projects: [missingAlpha],
				agents: [alphaOlder, alphaNewer],
				projectSessionsByDirectory: new Map([[alpha.directory, [alphaOlder, alphaNewer]]]),
				preferences,
			}),
		).toEqual([{ type: "project", project: missingAlpha, sessions: [alphaNewer, alphaOlder] }])
	})

	test("groups sessions onto projects even when path separators differ", () => {
		const windowsProject = project("devo", "C:\\Users\\lenovo\\Desktop\\devo", 200)
		const session = {
			...agent("slash-session", windowsProject, 10, 30),
			directory: "C:/Users/lenovo/Desktop/devo",
			projectDirectory: "C:/Users/lenovo/Desktop/devo",
		}

		expect(groupAgentsByProject([session], [windowsProject]).get(windowsProject.directory)).toEqual([
			session,
		])
	})
})
