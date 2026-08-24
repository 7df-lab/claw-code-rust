import { describe, expect, mock, test } from "bun:test"
import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"
import { renderToStaticMarkup } from "react-dom/server"
import type { Agent, SidebarProject } from "../../lib/types"

mock.module("@tanstack/react-router", () => ({
	useNavigate: () => () => undefined,
}))

const { ProjectRow, SessionRow } = await import("./sidebar-rows")

function agent(): Agent {
	return {
		id: "session-1",
		sessionId: "session-1",
		name: "Greeting and Introduction",
		status: "idle",
		environment: "local",
		project: "devo",
		projectSlug: "devo-123",
		directory: "C:\\Users\\lenovo\\Desktop\\devo",
		projectDirectory: "C:\\Users\\lenovo\\Desktop\\devo",
		branch: "main",
		duration: "42m",
		activities: [],
		permissions: [],
		questions: [],
		createdAt: 1,
		lastActiveAt: 2,
	}
}

function project(): SidebarProject {
	return {
		id: "project-1",
		slug: "devo-123",
		name: "devo",
		directory: "/Users/tsiao/Desktop/devo",
		agentCount: 1,
		lastActiveAt: 2,
		hasActiveAgent: false,
		folderStatus: "missing",
	}
}

describe("SessionRow", () => {
	test("keeps the selected row visually stable on hover", () => {
		const markup = renderToStaticMarkup(
			<SessionRow
				agent={agent()}
				isSelected
				onRename={async () => {}}
				onDelete={async () => {}}
				onFork={async () => {}}
			/>,
		)

		expect({
			hasSelectedBackground: markup.includes("bg-black/[0.07]"),
			hasLightHoverBackground: markup.includes("hover:bg-black/[0.04]"),
			hasDarkHoverBackground: markup.includes("dark:hover:bg-white/[0.06]"),
			hidesStatusOnHover: markup.includes("group-hover/sidebar-row:opacity-0"),
			hidesStatusWhenMenuOpen: markup.includes("group-has-[[data-popup-open]]/sidebar-row:opacity-0"),
			showsActionsOnHover: markup.includes("group-hover/sidebar-row:opacity-100"),
			showsActionsWhenMenuOpen: markup.includes("group-has-[[data-popup-open]]/sidebar-row:opacity-100"),
		}).toEqual({
			hasSelectedBackground: true,
			hasLightHoverBackground: false,
			hasDarkHoverBackground: false,
			hidesStatusOnHover: true,
			hidesStatusWhenMenuOpen: true,
			showsActionsOnHover: true,
			showsActionsWhenMenuOpen: true,
		})
	})

	test("keeps hover affordances for unselected rows", () => {
		const markup = renderToStaticMarkup(
			<SessionRow
				agent={agent()}
				isSelected={false}
				onRename={async () => {}}
				onDelete={async () => {}}
				onFork={async () => {}}
			/>,
		)

		expect({
			hasLightHoverBackground: markup.includes("hover:bg-black/[0.04]"),
			hasDarkHoverBackground: markup.includes("dark:hover:bg-white/[0.06]"),
			hidesStatusOnHover: markup.includes("group-hover/sidebar-row:opacity-0"),
			hidesStatusWhenMenuOpen: markup.includes("group-has-[[data-popup-open]]/sidebar-row:opacity-0"),
			showsActionsOnHover: markup.includes("group-hover/sidebar-row:opacity-100"),
			fadesStatusAndMenu: markup.includes("transition-[background-color,color,opacity] duration-150"),
		}).toEqual({
			hasLightHoverBackground: true,
			hasDarkHoverBackground: true,
			hidesStatusOnHover: true,
			hidesStatusWhenMenuOpen: true,
			showsActionsOnHover: true,
			fadesStatusAndMenu: false,
		})
	})

	test("renders last active text from timestamp instead of cached duration", () => {
		const originalNow = Date.now
		Date.now = () => Date.parse("2026-06-24T02:00:00.000Z")
		try {
			const staleAgent = {
				...agent(),
				duration: "now",
				lastActiveAt: Date.parse("2026-06-24T00:00:00.000Z"),
			}

			const markup = renderToStaticMarkup(
				<SessionRow
					agent={staleAgent}
					isSelected={false}
					onRename={async () => {}}
					onDelete={async () => {}}
					onFork={async () => {}}
				/>,
			)

			expect({
				usesTimestamp: markup.includes(">2h<"),
				usesCachedDuration: markup.includes(">now<"),
			}).toEqual({
				usesTimestamp: true,
				usesCachedDuration: false,
			})
		} finally {
			Date.now = originalNow
		}
	})

	test("renders a spinner while the session is running instead of an unread dot", () => {
		const markup = renderToStaticMarkup(
			<SessionRow
				agent={{ ...agent(), status: "running" }}
				isSelected={false}
				onRename={async () => {}}
				onDelete={async () => {}}
				onFork={async () => {}}
			/>,
		)

		expect({
			hasSpinner: markup.includes("animate-spin"),
			hasLoaderIcon: markup.includes("lucide-loader-circle"),
			hasCustomRing: markup.includes("border-[1.5px]"),
			hasBlueDot: markup.includes("size-2 rounded-full bg-[#3396f4]"),
		}).toEqual({
			hasSpinner: true,
			hasLoaderIcon: true,
			hasCustomRing: false,
			hasBlueDot: false,
		})
	})

	test("keeps compact status indicators while the session is waiting", () => {
		const markup = renderToStaticMarkup(
			<SessionRow
				agent={{ ...agent(), status: "waiting" }}
				isSelected={false}
				onRename={async () => {}}
				onDelete={async () => {}}
				onFork={async () => {}}
			/>,
		)

		expect({
			hasBlueDot: markup.includes("size-2 rounded-full bg-[#3396f4]"),
			hasSpinner: markup.includes("animate-spin"),
		}).toEqual({
			hasBlueDot: true,
			hasSpinner: false,
		})
	})

	test("keeps last active text for idle rows", () => {
		const originalNow = Date.now
		Date.now = () => Date.parse("2026-06-24T02:00:00.000Z")
		try {
			const markup = renderToStaticMarkup(
				<SessionRow
					agent={{
						...agent(),
						status: "idle",
						lastActiveAt: Date.parse("2026-06-24T00:00:00.000Z"),
					}}
					isSelected={false}
					onRename={async () => {}}
					onDelete={async () => {}}
					onFork={async () => {}}
				/>,
			)

			expect({
				hasBlueDot: markup.includes("size-2 rounded-full bg-[#3396f4]"),
				showsRelativeTime: markup.includes(">2h<"),
			}).toEqual({
				hasBlueDot: false,
				showsRelativeTime: true,
			})
		} finally {
			Date.now = originalNow
		}
	})

	test("renders a spinning icon for running rows and last active text for idle rows", () => {
		const originalNow = Date.now
		Date.now = () => Date.parse("2026-06-24T02:00:00.000Z")
		try {
			const idleAgent = {
				...agent(),
				status: "idle" as const,
				lastActiveAt: Date.parse("2026-06-24T00:00:00.000Z"),
			}
			const runningAgent = {
				...idleAgent,
				status: "running" as const,
			}

			const idleMarkup = renderToStaticMarkup(
				<SessionRow
					agent={idleAgent}
					isSelected={false}
					onRename={async () => {}}
					onDelete={async () => {}}
					onFork={async () => {}}
				/>,
			)
			const runningMarkup = renderToStaticMarkup(
				<SessionRow
					agent={runningAgent}
					isSelected={false}
					onRename={async () => {}}
					onDelete={async () => {}}
					onFork={async () => {}}
				/>,
			)

			expect({
				idleShowsLastActive: idleMarkup.includes(">2h<"),
				idleSpins: idleMarkup.includes("animate-spin"),
				runningUsesLoader: runningMarkup.includes("lucide-loader-circle"),
				runningSpins: runningMarkup.includes("animate-spin"),
				runningShowsLastActive: runningMarkup.includes(">2h<"),
			}).toEqual({
				idleShowsLastActive: true,
				idleSpins: false,
				runningUsesLoader: true,
				runningSpins: true,
				runningShowsLastActive: false,
			})
		} finally {
			Date.now = originalNow
		}
	})

	test("marks missing project rows and sessions as unavailable", () => {
		const projectMarkup = renderToStaticMarkup(
			<ProjectRow
				project={project()}
				isSelected={false}
				showCount={false}
				isCollapsed={false}
				canToggleSessions
				onSelect={() => {}}
				isUnavailable
			/>,
		)
		const sessionMarkup = renderToStaticMarkup(
			<SessionRow
				agent={agent()}
				isSelected={false}
				onRename={async () => {}}
				onDelete={async () => {}}
				onFork={async () => {}}
				projectUnavailable
			/>,
		)

		expect({
			projectAriaDisabled: projectMarkup.includes('aria-disabled="true"'),
			projectUnavailableData: projectMarkup.includes('data-folder-unavailable="true"'),
			sessionAriaDisabled: sessionMarkup.includes('aria-disabled="true"'),
			sessionUnavailableData: sessionMarkup.includes('data-folder-unavailable="true"'),
		}).toEqual({
			projectAriaDisabled: true,
			projectUnavailableData: true,
			sessionAriaDisabled: true,
			sessionUnavailableData: true,
		})
	})

	test("rename field inherits the session title typeface", async () => {
		const source = await readFile(
			join(dirname(fileURLToPath(import.meta.url)), "sidebar-rows.tsx"),
			"utf8",
		)

		expect({
			sharesTitleTextClass: source.includes("sessionTitleTextClass"),
			inheritsFont: source.includes('className="min-w-0 flex-1 appearance-none border-0 bg-transparent p-0 [font:inherit] text-inherit outline-none"'),
			avoidsSharedInput: !source.includes('from "@devo/ui/components/input"'),
		}).toEqual({
			sharesTitleTextClass: true,
			inheritsFont: true,
			avoidsSharedInput: true,
		})
	})
})
