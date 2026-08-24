import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const settingsPageSource = readFileSync(new URL("./settings-page.tsx", import.meta.url), "utf8")
const mcpSource = readFileSync(new URL("./mcp-settings.tsx", import.meta.url), "utf8")
const skillSource = readFileSync(new URL("./skill-settings.tsx", import.meta.url), "utf8")
const ruleSource = readFileSync(new URL("./rule-settings.tsx", import.meta.url), "utf8")
const customizeSource = readFileSync(new URL("../customize/customize-view.tsx", import.meta.url), "utf8")
const sidebarSource = readFileSync(new URL("../sidebar/app-sidebar-content.tsx", import.meta.url), "utf8")
const layoutSource = readFileSync(new URL("../sidebar-layout.tsx", import.meta.url), "utf8")
const routerSource = readFileSync(new URL("../../router.tsx", import.meta.url), "utf8")
const menuSource = readFileSync(new URL("../../../main/index.ts", import.meta.url), "utf8")

describe("Desktop MCP and Skills settings", () => {
	test("registers MCP and Skills settings tabs and routes", () => {
		expect({
			mcpTab: settingsPageSource.includes('id: "mcp"') && settingsPageSource.includes('label: "MCP"'),
			skillsTab:
				settingsPageSource.includes('id: "skills"') && settingsPageSource.includes('label: "Skills"'),
			mcpRoute: routerSource.includes('path: "mcp"') && routerSource.includes("McpSettings"),
			skillsRoute: routerSource.includes('path: "skills"') && routerSource.includes("SkillSettings"),
			listsMcp: mcpSource.includes("client.mcp.list()"),
			togglesMcp: mcpSource.includes("client.mcp.setEnabled"),
			listsMcpTools: mcpSource.includes("client.mcp.tools"),
			listsSkills: skillSource.includes("client.app.skills()"),
			togglesSkills: skillSource.includes("setSkillEnabled"),
		}).toEqual({
			mcpTab: true,
			skillsTab: true,
			mcpRoute: true,
			skillsRoute: true,
			listsMcp: true,
			togglesMcp: true,
			listsMcpTools: true,
			listsSkills: true,
			togglesSkills: true,
		})
	})

	test("registers a Customize pane for MCP, Skills, and Rules", () => {
		expect({
			sidebarEntry: sidebarSource.includes("Customize") && sidebarSource.includes("setCustomizeOpen(true)"),
			inContentPane: layoutSource.includes("<CustomizeView") && !routerSource.includes('path: "customize"'),
			customizeTabs:
				customizeSource.includes('id: "mcps"') &&
				customizeSource.includes('id: "skills"') &&
				customizeSource.includes('id: "rules"'),
			listsRules: ruleSource.includes("window.devo.rules.list"),
			createsRules: ruleSource.includes("window.devo.rules.create"),
			popsFileSubmenu: menuSource.includes("explicitSubmenu") && menuSource.includes("activePopupMenu"),
		}).toEqual({
			sidebarEntry: true,
			inContentPane: true,
			customizeTabs: true,
			listsRules: true,
			createsRules: true,
			popsFileSubmenu: true,
		})
	})

	test("File menu offers New Agent, Open Folder, and New Terminal", () => {
		expect({
			fileMenu: menuSource.includes('label: "File"'),
			newAgent: menuSource.includes('label: "New Agent"'),
			openFolder: menuSource.includes('label: "Open Folder"'),
			newTerminal: menuSource.includes('label: "New Terminal"'),
		}).toEqual({
			fileMenu: true,
			newAgent: true,
			openFolder: true,
			newTerminal: true,
		})
	})
})
