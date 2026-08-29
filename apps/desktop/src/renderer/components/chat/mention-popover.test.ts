import { describe, expect, test } from "bun:test"
import { readFileSync } from "node:fs"
import type { ReferenceSearchResult } from "@devo-ai/sdk/v2/client"
import { isMentionOptionDisabled, isMentionOptionVisible, mapReferenceSearchResults } from "./mention-popover"
import { createMentionFromOption, insertMentionIntoText } from "./prompt-mentions"

const mentionPopoverSource = readFileSync(new URL("./mention-popover.tsx", import.meta.url), "utf8")
const popoverStylesSource = readFileSync(
	new URL("./composer-popover-styles.ts", import.meta.url),
	"utf8",
)

describe("mention popover reference results", () => {
	test("preserves skill, MCP, and file results from the server", () => {
		const results: ReferenceSearchResult[] = [
			{
				kind: "skill",
				display_name: "openai-docs",
				description: "Use official OpenAI documentation",
				insert_text: "@openai-docs",
				mention_path: "skills/openai-docs/SKILL.md",
			},
			{
				kind: "mcp",
				display_name: "Docs",
				description: "Documentation server",
				insert_text: "@mcp:docs",
				mention_path: "mcp://server/docs",
				is_disabled: true,
				disabled_reason: "Server is disconnected",
			},
			{
				kind: "file",
				display_name: "src/main.rs",
				insert_text: "@main.rs",
				mention_path: "src/main.rs",
				file_path: "/workspace/src/main.rs",
			},
		]

		expect(mapReferenceSearchResults(results)).toEqual([
			{
				type: "skill",
				name: "openai-docs",
				display: "openai-docs",
				description: "Use official OpenAI documentation",
				insertText: "@openai-docs",
				mentionPath: "skills/openai-docs/SKILL.md",
				disabled: false,
				disabledReason: undefined,
			},
			{
				type: "mcp",
				name: "Docs",
				display: "Docs",
				description: "Documentation server",
				insertText: "@mcp:docs",
				mentionPath: "mcp://server/docs",
				disabled: true,
				disabledReason: "Server is disconnected",
			},
			{
				type: "file",
				path: "src/main.rs",
				display: "src/main.rs",
				insertText: "@main.rs",
				disabled: false,
				disabledReason: undefined,
			},
		])
	})

	test("inserts the exact server token for Skill and MCP selections", () => {
		const [skill, mcp] = mapReferenceSearchResults([
			{
				kind: "skill",
				display_name: "OpenAI Docs",
				insert_text: "@openai-docs",
				mention_path: "skills/openai-docs/SKILL.md",
			},
			{
				kind: "mcp",
				display_name: "Documentation",
				insert_text: "@mcp:docs",
				mention_path: "mcp://server/docs",
			},
		])

		expect([
			insertMentionIntoText("Ask @open", 9, createMentionFromOption(skill)),
			insertMentionIntoText("Use @doc", 8, createMentionFromOption(mcp)),
		]).toEqual([
			{ text: "Ask @openai-docs ", cursorPosition: 17 },
			{ text: "Use @mcp:docs ", cursorPosition: 14 },
		])
	})

	test("excludes references with a disabled reason from selection", () => {
		const [mcp] = mapReferenceSearchResults([
			{
				kind: "mcp",
				display_name: "Disconnected MCP",
				insert_text: "@mcp:disconnected",
				disabled_reason: "Server is disconnected",
			},
		])

		expect({ option: mcp, selectable: !isMentionOptionDisabled(mcp) }).toEqual({
			option: {
				type: "mcp",
				name: "Disconnected MCP",
				display: "Disconnected MCP",
				description: undefined,
				insertText: "@mcp:disconnected",
				mentionPath: undefined,
				disabled: true,
				disabledReason: "Server is disconnected",
			},
			selectable: false,
		})
	})

	test("hides disabled MCP servers from the popover list", () => {
		const options = mapReferenceSearchResults([
			{
				kind: "mcp",
				display_name: "Connected",
				insert_text: "@mcp:connected",
			},
			{
				kind: "mcp",
				display_name: "Disconnected",
				insert_text: "@mcp:disconnected",
				is_disabled: true,
				disabled_reason: "Server is disconnected",
			},
			{
				kind: "skill",
				display_name: "docs",
				insert_text: "@docs",
				description: "Lookup docs",
			},
		]).filter(isMentionOptionVisible)

		expect(options.map((option) => option.display)).toEqual(["Connected", "docs"])
	})

	test("treats camelCase wire disabled flags as disabled MCP", () => {
		const options = mapReferenceSearchResults([
			{
				kind: "mcp",
				display_name: "Wire Disabled",
				insert_text: "@mcp:wire",
				isDisabled: true,
				disabledReason: "Server is disconnected",
			} as ReferenceSearchResult & {
				isDisabled: boolean
				disabledReason: string
			},
		]).filter(isMentionOptionVisible)

		expect(options).toEqual([])
	})

	test("uses a single outer scroll container without nested ScrollArea", () => {
		expect({
			noScrollAreaImport: !mentionPopoverSource.includes("@devo/ui/components/scroll-area"),
			outerOverflowYAuto:
				popoverStylesSource.includes("overflow-y-auto") &&
				!popoverStylesSource.includes("scroll-area-viewport"),
			usesSharedScrollClass: mentionPopoverSource.includes("composerPopoverScrollClass"),
		}).toEqual({
			noScrollAreaImport: true,
			outerOverflowYAuto: true,
			usesSharedScrollClass: true,
		})
	})

	test("renders skill and MCP rows as a single compact line", () => {
		expect({
			skillMcpSingleLine: mentionPopoverSource.includes(
				'<span className="shrink-0 font-medium tracking-normal">{option.display}</span>',
			),
			noStackedSkillBody: !mentionPopoverSource.includes(
				'className="min-w-0 flex-1"',
			),
			filtersDisabledMcp: mentionPopoverSource.includes("isMentionOptionVisible"),
		}).toEqual({
			skillMcpSingleLine: true,
			noStackedSkillBody: true,
			filtersDisabledMcp: true,
		})
	})

	test("matches the shared minimal composer popover surface", () => {
		expect({
			usesSharedShell: mentionPopoverSource.includes("composerPopoverShellClass"),
			usesSharedItems: mentionPopoverSource.includes("composerPopoverItemClass"),
			usesMutedIcons: mentionPopoverSource.includes("composerPopoverIconClass"),
			omitsAccentIconColors:
				!mentionPopoverSource.includes("text-blue-400") &&
				!mentionPopoverSource.includes("text-cyan-500") &&
				!mentionPopoverSource.includes("text-fuchsia-500"),
			shellIsQuiet:
				popoverStylesSource.includes("shadow-sm") &&
				popoverStylesSource.includes("border-border/70") &&
				!popoverStylesSource.includes("shadow-md"),
			activeUsesMuted:
				popoverStylesSource.includes("bg-muted/80") &&
				!popoverStylesSource.includes("bg-accent"),
		}).toEqual({
			usesSharedShell: true,
			usesSharedItems: true,
			usesMutedIcons: true,
			omitsAccentIconColors: true,
			shellIsQuiet: true,
			activeUsesMuted: true,
		})
	})
})
