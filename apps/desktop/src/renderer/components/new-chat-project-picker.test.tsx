import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import type { SidebarProject } from "../lib/types"
import { NewChatProjectPicker } from "./new-chat-project-picker"

const newChatSource = readFileSync(new URL("./new-chat.tsx", import.meta.url), "utf8")

const project: SidebarProject = {
	id: "project-1",
	slug: "devo-1",
	name: "devo_desktop_0626",
	directory: "/Users/tester/devo_desktop_0626",
	agentCount: 0,
	lastActiveAt: 0,
	hasActiveAgent: false,
}

describe("NewChatProjectPicker", () => {
	test("renders the choose-project trigger", () => {
		const markup = renderToStaticMarkup(
			<NewChatProjectPicker
				projects={[project]}
				selectedProject={undefined}
				selectedDirectory=""
				onSelectProject={() => {}}
				onClearProject={() => {}}
				onStartFromScratch={() => {}}
				onUseExistingFolder={async () => {}}
			/>,
		)

		expect({
			chooseProject: markup.includes("Choose project"),
			projectNameHiddenUntilOpen: markup.includes("devo_desktop_0626"),
			compactTriggerHeight: markup.includes("h-8"),
			tallTriggerHeightRemoved: !markup.includes("h-9"),
		}).toEqual({
			chooseProject: true,
			projectNameHiddenUntilOpen: false,
			compactTriggerHeight: true,
			tallTriggerHeightRemoved: true,
		})
	})

	test("keeps the new-chat project strip vertically compact", () => {
		expect({
			compactProjectStripPadding: newChatSource.includes('className="px-4 py-1"'),
			tallProjectStripPaddingRemoved: !newChatSource.includes('className="px-4 pb-3 pt-2"'),
		}).toEqual({
			compactProjectStripPadding: true,
			tallProjectStripPaddingRemoved: true,
		})
	})

	test("shows the selected project chip", () => {
		const markup = renderToStaticMarkup(
			<NewChatProjectPicker
				projects={[project]}
				selectedProject={project}
				selectedDirectory={project.directory}
				onSelectProject={() => {}}
			/>,
		)

		expect({
			projectName: markup.includes("devo_desktop_0626"),
			clearProjectButton: markup.includes('aria-label="Clear project"'),
		}).toEqual({
			projectName: true,
			clearProjectButton: false,
		})
	})

	test("optional clear control appears only when provided", () => {
		const markup = renderToStaticMarkup(
			<NewChatProjectPicker
				projects={[project]}
				selectedProject={project}
				selectedDirectory={project.directory}
				onSelectProject={() => {}}
				onClearProject={() => {}}
			/>,
		)

		expect(markup.includes('aria-label="Clear project"')).toBe(true)
	})

	test("new chat keeps a project selected instead of clearing it", () => {
		expect(newChatSource.includes("onClearProject")).toBe(false)
	})
})
