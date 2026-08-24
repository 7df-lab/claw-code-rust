import { describe, expect, test } from "bun:test"
import { navigateToNewChat, resolveNewChatRoute, resolveSelectedProjectDirectory } from "./project-selection"
import type { SidebarProject } from "./types"

function project(
	name: string,
	directory: string,
	lastActiveAt: number,
	folderStatus?: SidebarProject["folderStatus"],
): SidebarProject {
	return {
		id: name,
		slug: `${name}-slug`,
		name,
		directory,
		agentCount: 1,
		lastActiveAt,
		hasActiveAgent: false,
		folderStatus,
	}
}

describe("new chat project selection", () => {
	test("keeps the current directory while projects are still loading", () => {
		expect(
			resolveSelectedProjectDirectory([], undefined, "/repo/alpha", {
				lastUsedDirectory: "/repo/alpha",
			}),
		).toBe("/repo/alpha")
	})

	test("uses the route project even when root has newer sessions", () => {
		const projects = [
			project("/", "/", 300),
			project("devo", "/Users/tsiao/Desktop/devo", 200),
		]

		expect(resolveSelectedProjectDirectory(projects, "devo-slug", "")).toBe(
			"/Users/tsiao/Desktop/devo",
		)
	})

	test("keeps an explicit project choice when project activity changes", () => {
		const projects = [
			project("/", "/", 300),
			project("devo_feat_desktop", "/Users/tsiao/Desktop/devo_feat_desktop", 200),
		]

		expect(
			resolveSelectedProjectDirectory(
				projects,
				undefined,
				"/Users/tsiao/Desktop/devo_feat_desktop",
				{ preserveCurrentDirectory: true },
			),
		).toBe("/Users/tsiao/Desktop/devo_feat_desktop")
	})

	test("falls back to the most recently active project on the root route", () => {
		const projects = [
			project("devo_feat_desktop", "/Users/tsiao/Desktop/devo_feat_desktop", 400),
			project("Desktop", "/Users/tsiao/Desktop", 300),
		]

		expect(
			resolveSelectedProjectDirectory(projects, undefined, "/Users/tsiao/Desktop", {
				preserveCurrentDirectory: false,
			}),
		).toBe("/Users/tsiao/Desktop/devo_feat_desktop")
	})

	test("prefers the last-used project over a newer idle project", () => {
		const projects = [
			project("alpha", "/repo/alpha", 400),
			project("beta", "/repo/beta", 100),
		]

		expect(
			resolveSelectedProjectDirectory(projects, undefined, "", {
				lastUsedDirectory: "/repo/beta",
			}),
		).toBe("/repo/beta")
	})

	test("does not default to a filesystem root when real projects exist", () => {
		const projects = [
			project("/", "/", 300),
			project("devo_simplify_0623", "/Users/tsiao/Desktop/devo_simplify_0623", 0),
		]

		expect(resolveSelectedProjectDirectory(projects, undefined, "")).toBe(
			"/Users/tsiao/Desktop/devo_simplify_0623",
		)
	})

	test("skips unavailable projects when choosing a default", () => {
		const projects = [
			project("old-worktree", "/Users/tsiao/Desktop/devo_missing", 400),
			project("devo", "/Users/tsiao/Desktop/devo", 300),
		]

		expect(
			resolveSelectedProjectDirectory(projects, undefined, "", {
				unavailableDirectories: new Set(["/Users/tsiao/Desktop/devo_missing"]),
			}),
		).toBe("/Users/tsiao/Desktop/devo")
	})

	test("keeps an explicitly routed unavailable project", () => {
		const projects = [
			project("old-worktree", "/Users/tsiao/Desktop/devo_missing", 400),
			project("devo", "/Users/tsiao/Desktop/devo", 300),
		]

		expect(
			resolveSelectedProjectDirectory(projects, "old-worktree-slug", "", {
				unavailableDirectories: new Set(["/Users/tsiao/Desktop/devo_missing"]),
			}),
		).toBe("/Users/tsiao/Desktop/devo_missing")
	})

	test("routes New chat into the current or last-used project", () => {
		const projects = [
			project("alpha", "/repo/alpha", 10),
			project("beta", "/repo/beta", 90),
		]

		expect(resolveNewChatRoute(projects, "alpha-slug", null)).toEqual({
			to: "/project/$projectSlug",
			params: { projectSlug: "alpha-slug" },
		})
		expect(resolveNewChatRoute(projects, undefined, "/repo/alpha")).toEqual({
			to: "/project/$projectSlug",
			params: { projectSlug: "alpha-slug" },
		})
		expect(resolveNewChatRoute(projects, undefined, null)).toEqual({
			to: "/project/$projectSlug",
			params: { projectSlug: "beta-slug" },
		})

		const navigated: Array<{ to: string; params?: Record<string, string> }> = []
		navigateToNewChat(
			(opts) => {
				navigated.push(opts)
			},
			projects,
			undefined,
			"/repo/alpha",
		)
		expect(navigated).toEqual([
			{ to: "/project/$projectSlug", params: { projectSlug: "alpha-slug" } },
		])
	})
})
