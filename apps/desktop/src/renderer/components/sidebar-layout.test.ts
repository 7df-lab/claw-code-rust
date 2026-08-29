import { describe, expect, test } from "bun:test"
import { readFile } from "node:fs/promises"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

const sourcePath = join(dirname(fileURLToPath(import.meta.url)), "sidebar-layout.tsx")

describe("sidebar layout window controls", () => {
	test("sidebar toggle has an accessible name", async () => {
		const source = await readFile(sourcePath, "utf8")

		expect(source).toContain('aria-label="Toggle sidebar"')
	})

	test("sidebar toggle uses the shared panel icon resource", async () => {
		const source = await readFile(sourcePath, "utf8")

		expect({
			importsSharedIcon: source.includes('import { LeftPanelIcon } from "./panel-icons"'),
			rendersSharedIcon: source.includes("<LeftPanelIcon"),
			replacesLucideIcon: !source.includes("PanelLeftIcon"),
		}).toEqual({
			importsSharedIcon: true,
			rendersSharedIcon: true,
			replacesLucideIcon: true,
		})
	})

	test("sidebar toggle matches macOS traffic light alignment and compact icon scale", async () => {
		const source = await readFile(sourcePath, "utf8")

		expect({
			definesMacAlignedTop: source.includes(
				"const WINDOW_CONTROLS_TOP = isMac && isElectronEnv ? 7 : 6",
			),
			usesTopConstant: source.includes("top: WINDOW_CONTROLS_TOP"),
			usesCompactPanelIcon: source.includes('className="size-3.5"'),
		}).toEqual({
			definesMacAlignedTop: true,
			usesTopConstant: true,
			usesCompactPanelIcon: true,
		})
	})

	test("windows app menu includes File actions", async () => {
		const source = await readFile(sourcePath, "utf8")
		expect({
			fileMenu: source.includes('{ id: "file", label: "File" }'),
			handlesNewAgent: source.includes('action === "new-agent"'),
			routesNewAgentWithProject: source.includes("navigateToNewChat(navigate, projects, projectSlug, lastProjectDirectory)"),
			handlesOpenFolder: source.includes('action === "open-folder"'),
			handlesNewTerminal: source.includes('action === "new-terminal"'),
			opensTerminal: source.includes("openNewTerminal()"),
		}).toEqual({
			fileMenu: true,
			handlesNewAgent: true,
			routesNewAgentWithProject: true,
			handlesOpenFolder: true,
			handlesNewTerminal: true,
			opensTerminal: true,
		})
	})

	test("sidebar width is drag-resizable and persisted", async () => {
		const source = await readFile(sourcePath, "utf8")
		expect({
			importsResizeHandle: source.includes(
				'import { SidebarResizeHandle } from "./sidebar/sidebar-resize-handle"',
			),
			importsWidthAtom: source.includes("sidebarWidthAtom"),
			appliesSidebarWidthVar: source.includes('"--sidebar-width"'),
			marksResizing: source.includes('data-resizing={sidebarResizing ? "true" : undefined}'),
			rendersHandle: source.includes("<SidebarResizeHandle"),
		}).toEqual({
			importsResizeHandle: true,
			importsWidthAtom: true,
			appliesSidebarWidthVar: true,
			marksResizing: true,
			rendersHandle: true,
		})
	})
})
