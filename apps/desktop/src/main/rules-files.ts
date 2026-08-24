import { existsSync, writeFileSync } from "node:fs"
import { homedir } from "node:os"
import { basename, dirname, join } from "node:path"

export const RULE_FILENAMES = ["AGENTS.override.md", "AGENTS.md", "CLAUDE.md", "PROMPT.md"] as const

export interface RuleFileInfo {
	path: string
	name: string
	directory: string
	scope: "user" | "project"
}

function userRuleCandidates(): string[] {
	const home = homedir()
	return [join(home, ".devo", "AGENTS.md"), join(home, ".config", "devo", "AGENTS.md")]
}

export function listRuleFiles(projectDirectories: string[]): RuleFileInfo[] {
	const seen = new Set<string>()
	const files: RuleFileInfo[] = []

	const add = (filePath: string, scope: RuleFileInfo["scope"]) => {
		if (!existsSync(filePath) || seen.has(filePath)) return
		seen.add(filePath)
		files.push({
			path: filePath,
			name: basename(filePath),
			directory: dirname(filePath),
			scope,
		})
	}

	for (const filePath of userRuleCandidates()) {
		add(filePath, "user")
	}
	for (const directory of projectDirectories) {
		if (!directory) continue
		for (const name of RULE_FILENAMES) {
			add(join(directory, name), "project")
		}
	}
	return files
}

export function createProjectAgentsMd(directory: string): RuleFileInfo {
	const filePath = join(directory, "AGENTS.md")
	if (!existsSync(filePath)) {
		writeFileSync(
			filePath,
			"# AGENTS.md\n\nProject instructions for Devo agents.\n",
			"utf-8",
		)
	}
	return {
		path: filePath,
		name: "AGENTS.md",
		directory,
		scope: "project",
	}
}
