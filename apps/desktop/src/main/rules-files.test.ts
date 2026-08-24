import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { describe, expect, test } from "bun:test"
import { createProjectAgentsMd, listRuleFiles } from "./rules-files"

describe("rule file discovery", () => {
	test("lists project AGENTS.md files and creates a stub when missing", () => {
		const root = mkdtempSync(join(tmpdir(), "devo-rules-"))
		try {
			const project = join(root, "repo")
			mkdirSync(project)
			writeFileSync(join(project, "CLAUDE.md"), "legacy", "utf-8")

			expect(listRuleFiles([project]).filter((file) => file.scope === "project")).toEqual([
				{
					path: join(project, "CLAUDE.md"),
					name: "CLAUDE.md",
					directory: project,
					scope: "project",
				},
			])

			expect(createProjectAgentsMd(project)).toEqual({
				path: join(project, "AGENTS.md"),
				name: "AGENTS.md",
				directory: project,
				scope: "project",
			})
			expect(
				listRuleFiles([project])
					.filter((file) => file.scope === "project")
					.map((file) => file.name),
			).toEqual(["AGENTS.md", "CLAUDE.md"])
		} finally {
			rmSync(root, { recursive: true, force: true })
		}
	})
})
