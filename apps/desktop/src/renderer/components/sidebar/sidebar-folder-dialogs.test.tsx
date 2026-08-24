import { describe, expect, test } from "bun:test"
import { renderToStaticMarkup } from "react-dom/server"
import type { SidebarProject } from "../../lib/types"
import {
	FolderRemoveDialogBody,
	MissingFolderDialogBody,
} from "./sidebar-folder-dialogs"

const project: SidebarProject = {
	id: "project-1",
	slug: "devo-1",
	name: "devo",
	directory: "/Users/tester/devo",
	agentCount: 0,
	lastActiveAt: 0,
	hasActiveAgent: false,
}

describe("sidebar folder dialogs", () => {
	test("explains remove deletes all sessions in the folder, not the disk folder", () => {
		const markup = renderToStaticMarkup(
			<FolderRemoveDialogBody
				project={project}
				pending={false}
				error={null}
				onCancel={() => {}}
				onConfirm={() => {}}
			/>,
		)

		expect({
			title: markup.includes("Remove folder from Devo Desktop"),
			deletesSessions: markup.includes("permanently deletes all sessions in this folder"),
			cannotUndo: markup.includes("cannot be undone"),
			keepsDiskFolder: markup.includes("The folder on disk will not be deleted"),
		}).toEqual({
			title: true,
			deletesSessions: true,
			cannotUndo: true,
			keepsDiskFolder: true,
		})
	})

	test("asks whether to remove missing folders from Devo Desktop", () => {
		const markup = renderToStaticMarkup(
			<MissingFolderDialogBody
				project={project}
				pending={false}
				error={null}
				onCancel={() => {}}
				onConfirmRemove={() => {}}
			/>,
		)

		expect({
			title: markup.includes("Folder no longer exists"),
			removeQuestion: markup.includes("Remove it from Devo Desktop"),
			deletesSessions: markup.includes("permanently deletes all sessions in this folder"),
		}).toEqual({
			title: true,
			removeQuestion: true,
			deletesSessions: true,
		})
	})
})
