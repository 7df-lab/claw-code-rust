import { describe, expect, test } from "bun:test"
import type { MenuItemConstructorOptions } from "electron"
import { buildDevoTrayMenuTemplate } from "./tray-menu"

function menuShape(items: MenuItemConstructorOptions[]): unknown[] {
	return items.map((item) => ({
		label: item.label,
		sublabel: item.sublabel,
		enabled: item.enabled,
		type: item.type,
		click: typeof item.click === "function",
		submenu: Array.isArray(item.submenu) ? menuShape(item.submenu) : undefined,
	}))
}

describe("buildDevoTrayMenuTemplate", () => {
	test("builds a Devo tray menu with running, recent, and actions", () => {
		const liveSessions = new Map([
			[
				"s1",
				{
					status: "busy",
					title: "修复 desktop Native 渲染和回复问题",
					directory: "/Users/tsiao/Desktop/devo_feat_desktop",
				},
			],
		])
		const sessions = [
			{
				id: "s1",
				title: "修复 desktop Native 渲染和回复问题",
				directory: "/Users/tsiao/Desktop/devo_feat_desktop",
				time: { created: 1000, updated: 5000 },
				totalInputTokens: 18_000,
				totalOutputTokens: 6_500,
				totalTokens: 24_500,
				totalCacheReadTokens: 1_200,
			},
			{
				id: "s2",
				title: "添加 macOS tray 图标",
				directory: "/Users/tsiao/Desktop/devo_feat_desktop",
				time: { created: 1000, updated: 4000 },
				totalInputTokens: 1_000,
				totalOutputTokens: 500,
				totalTokens: 1_500,
			},
			{
				id: "s3",
				title: "调研 devo 被杀原因",
				directory: "/Users/tsiao/Desktop/devo_simplify_0623",
				time: { created: 1000, updated: 3000 },
				totalInputTokens: 2_000,
				totalOutputTokens: 800,
				totalTokens: 2_800,
			},
			{
				id: "s4",
				title: "梳理深度研究流程",
				directory: "/Users/tsiao/Desktop/devo_simplify_0623",
				time: { created: 1000, updated: 2000 },
				totalInputTokens: 3_000,
				totalOutputTokens: 900,
				totalTokens: 3_900,
			},
			{
				id: "s5",
				title: "Create feat/desktop worktree",
				directory: "/Users/tsiao/Desktop/devo_simplify_0623",
				time: { created: 1000, updated: 1000 },
				totalInputTokens: 4_000,
				totalOutputTokens: 1_100,
				totalTokens: 5_100,
			},
		]

		const template = buildDevoTrayMenuTemplate({
			liveSessions,
			discovery: {
				projects: [],
				sessions,
			},
			onNavigateToSession: () => {},
			onNewChat: () => {},
			onOpenDevo: () => {},
			onQuitDevo: () => {},
			pendingCount: 0,
		})

		expect(menuShape(template)).toEqual([
			{ label: "Running", sublabel: undefined, enabled: false, type: undefined, click: false, submenu: undefined },
			{
				label: "修复 desktop Native 渲染和回复问题",
				sublabel: "devo_feat_desktop",
				enabled: undefined,
				type: undefined,
				click: true,
				submenu: undefined,
			},
			{ label: undefined, sublabel: undefined, enabled: undefined, type: "separator", click: false, submenu: undefined },
			{ label: "Recent", sublabel: undefined, enabled: false, type: undefined, click: false, submenu: undefined },
			{
				label: "添加 macOS tray 图标",
				sublabel: "devo_feat_desktop",
				enabled: undefined,
				type: undefined,
				click: true,
				submenu: undefined,
			},
			{
				label: "调研 devo 被杀原因",
				sublabel: "devo_simplify_0623",
				enabled: undefined,
				type: undefined,
				click: true,
				submenu: undefined,
			},
			{
				label: "梳理深度研究流程",
				sublabel: "devo_simplify_0623",
				enabled: undefined,
				type: undefined,
				click: true,
				submenu: undefined,
			},
			{
				label: "More",
				sublabel: undefined,
				enabled: undefined,
				type: undefined,
				click: false,
				submenu: [
					{
						label: "Create feat/desktop worktree",
						sublabel: "devo_simplify_0623",
						enabled: undefined,
						type: undefined,
						click: true,
						submenu: undefined,
					},
				],
			},
			{ label: undefined, sublabel: undefined, enabled: undefined, type: "separator", click: false, submenu: undefined },
			{ label: "New Chat", sublabel: undefined, enabled: undefined, type: undefined, click: true, submenu: undefined },
			{ label: undefined, sublabel: undefined, enabled: undefined, type: "separator", click: false, submenu: undefined },
			{ label: "Open Devo", sublabel: undefined, enabled: undefined, type: undefined, click: true, submenu: undefined },
			{ label: undefined, sublabel: undefined, enabled: undefined, type: "separator", click: false, submenu: undefined },
			{ label: "Quit Devo", sublabel: undefined, enabled: undefined, type: undefined, click: true, submenu: undefined },
		])
	})

	test("keeps idle live sessions out of Running and lists them under Recent", () => {
		const liveSessions = new Map([
			[
				"s1",
				{
					status: "idle",
					title: "Idle live session",
					directory: "/Users/tsiao/Desktop/devo_feat_desktop",
				},
			],
			[
				"s2",
				{
					status: "busy",
					title: "Still working",
					directory: "/Users/tsiao/Desktop/devo_feat_desktop",
				},
			],
		])
		const sessions = [
			{
				id: "s1",
				title: "Idle live session",
				directory: "/Users/tsiao/Desktop/devo_feat_desktop",
				time: { created: 1000, updated: 5000 },
			},
			{
				id: "s2",
				title: "Still working",
				directory: "/Users/tsiao/Desktop/devo_feat_desktop",
				time: { created: 1000, updated: 4000 },
			},
			{
				id: "s3",
				title: "Older idle chat",
				directory: "/Users/tsiao/Desktop/devo_simplify_0623",
				time: { created: 1000, updated: 3000 },
			},
		]

		const template = buildDevoTrayMenuTemplate({
			liveSessions,
			discovery: {
				projects: [],
				sessions,
			},
			onNavigateToSession: () => {},
			onNewChat: () => {},
			onOpenDevo: () => {},
			onQuitDevo: () => {},
			pendingCount: 0,
		})

		expect(menuShape(template)).toEqual([
			{ label: "Running", sublabel: undefined, enabled: false, type: undefined, click: false, submenu: undefined },
			{
				label: "Still working",
				sublabel: "devo_feat_desktop",
				enabled: undefined,
				type: undefined,
				click: true,
				submenu: undefined,
			},
			{ label: undefined, sublabel: undefined, enabled: undefined, type: "separator", click: false, submenu: undefined },
			{ label: "Recent", sublabel: undefined, enabled: false, type: undefined, click: false, submenu: undefined },
			{
				label: "Idle live session",
				sublabel: "devo_feat_desktop",
				enabled: undefined,
				type: undefined,
				click: true,
				submenu: undefined,
			},
			{
				label: "Older idle chat",
				sublabel: "devo_simplify_0623",
				enabled: undefined,
				type: undefined,
				click: true,
				submenu: undefined,
			},
			{ label: undefined, sublabel: undefined, enabled: undefined, type: "separator", click: false, submenu: undefined },
			{ label: "New Chat", sublabel: undefined, enabled: undefined, type: undefined, click: true, submenu: undefined },
			{ label: undefined, sublabel: undefined, enabled: undefined, type: "separator", click: false, submenu: undefined },
			{ label: "Open Devo", sublabel: undefined, enabled: undefined, type: undefined, click: true, submenu: undefined },
			{ label: undefined, sublabel: undefined, enabled: undefined, type: "separator", click: false, submenu: undefined },
			{ label: "Quit Devo", sublabel: undefined, enabled: undefined, type: undefined, click: true, submenu: undefined },
		])
	})
})
