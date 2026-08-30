import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const popoverSource = readFileSync(new URL("./slash-command-popover.tsx", import.meta.url), "utf8")
const chipSource = readFileSync(new URL("./composer-mode-chip.tsx", import.meta.url), "utf8")
const popoverStylesSource = readFileSync(
	new URL("./composer-popover-styles.ts", import.meta.url),
	"utf8",
)
const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
const newChatSource = readFileSync(new URL("../new-chat.tsx", import.meta.url), "utf8")
const desktopChromeSource = readFileSync(new URL("../../desktop-chrome.css", import.meta.url), "utf8")
const desktopAgentsSource = readFileSync(new URL("../../../../AGENTS.md", import.meta.url), "utf8")
const clientCommandsBlock = popoverSource.match(/const CLIENT_COMMANDS[\s\S]*?\n\]/)?.[0] ?? ""

describe("Desktop slash command composer", () => {
	test("limits the popover to first-party composer commands", () => {
		expect({
			showsCompact: clientCommandsBlock.includes('name: "compact"'),
			showsGoal: clientCommandsBlock.includes('name: "goal"'),
			showsPlan: clientCommandsBlock.includes('name: "plan"'),
			showsResearch: clientCommandsBlock.includes('name: "research"'),
			omitsUndo: !clientCommandsBlock.includes('name: "undo"'),
			showsSkills: clientCommandsBlock.includes('name: "skills"'),
			omitsServerCommands: !popoverSource.includes("useServerCommands"),
			omitsUserCommandDispatch: !chatViewSource.includes("session.command"),
			omitsSearchHeader:
				!popoverSource.includes("SearchIcon") && !popoverSource.includes("Commands</span>"),
			usesSharedPopoverShell: popoverSource.includes("composerPopoverShellClass"),
			usesSharedPopoverItem: popoverSource.includes("composerPopoverItemClass"),
			singleOuterScroll:
				!popoverSource.includes("@devo/ui/components/scroll-area") &&
				popoverStylesSource.includes("overflow-y-auto"),
		}).toEqual({
			showsCompact: true,
			showsGoal: true,
			showsPlan: true,
			showsResearch: true,
			omitsUndo: true,
			showsSkills: true,
			omitsServerCommands: true,
			omitsUserCommandDispatch: true,
			omitsSearchHeader: true,
			usesSharedPopoverShell: true,
			usesSharedPopoverItem: true,
			singleOuterScroll: true,
		})
	})

	test("turns goal into a footer trigger chip and plan into plan mode", () => {
		expect({
			requirementComment: chatViewSource.includes(
				"Desktop slash commands are limited to first-party",
			),
			chipComponent: chipSource.includes("export function ComposerModeChip"),
			goalFooterChip:
				chatViewSource.includes('variant="goal"') &&
				chatViewSource.includes("ComposerModeChip"),
			planModeBadge:
				chatViewSource.includes('variant="plan"') &&
				chatViewSource.includes("ComposerModeChip"),
			planTint: chipSource.includes("bg-amber-500/12"),
			goalIcon: chipSource.includes("GoalIcon"),
			planIcon: chipSource.includes("ListTodoIcon"),
			researchIcon: popoverSource.includes("MicroscopeIcon"),
			goalInsertText: popoverSource.includes('insertText: "/goal "'),
			planInsertText: popoverSource.includes('insertText: "/plan "'),
			researchInsertText: popoverSource.includes('insertText: "/research "'),
			researchPromptPath: chatViewSource.includes('case "research":'),
			closeOnlyOnHover:
				chipSource.includes("opacity-0") && chipSource.includes("group-hover:opacity-100"),
			closeReplacesIconInPlace:
				chipSource.includes("group-hover:opacity-0") &&
				chipSource.includes("group-hover:opacity-100"),
			hoverBackground: chipSource.includes("hover:bg-muted"),
			shiftTabToggle: chatViewSource.includes('e.key === "Tab" && e.shiftKey'),
			triggeredPrompt: chatViewSource.includes("text: `/${trigger} ${text.trim()}`"),
			iconStrokeMatchesSidebar:
				popoverSource.includes("composerPopoverIconClass") &&
				popoverStylesSource.includes("stroke-[1.5]") &&
				chipSource.includes("size-3.5 stroke-[1.5]"),
		}).toEqual({
			requirementComment: true,
			chipComponent: true,
			goalFooterChip: true,
			planModeBadge: true,
			planTint: true,
			goalIcon: true,
			planIcon: true,
			researchIcon: true,
			goalInsertText: true,
			planInsertText: true,
			researchInsertText: true,
			researchPromptPath: true,
			closeOnlyOnHover: true,
			closeReplacesIconInPlace: true,
			hoverBackground: true,
			shiftTabToggle: true,
			triggeredPrompt: true,
			iconStrokeMatchesSidebar: true,
		})
	})

	test("shows slash command suggestions on the new-session composer", () => {
		expect({
			importsPopover: newChatSource.includes("SlashCommandPopover"),
			detectsSlashTrigger: newChatSource.includes("onSlashChange"),
			delegatesSlashKeys: newChatSource.includes("slashPopoverRef.current?.handleKeyDown"),
			rendersPopover: newChatSource.includes("<SlashCommandPopover"),
			allowsPopoverOverflow:
				newChatSource.includes("data-popover-open") &&
				desktopChromeSource.includes('.devo-composer-shell[data-popover-open="true"]') &&
				desktopChromeSource.includes("overflow: visible"),
		}).toEqual({
			importsPopover: true,
			detectsSlashTrigger: true,
			delegatesSlashKeys: true,
			rendersPopover: true,
			allowsPopoverOverflow: true,
		})
	})

	test("documents desktop icon consistency for renderer work", () => {
		expect({
			documentsSidebarIconSizing: desktopAgentsSource.includes("size-3.5"),
			documentsSidebarIconStroke: desktopAgentsSource.includes("stroke-[1.5]"),
			documentsStableIconSlots: desktopAgentsSource.includes("swap the glyph in place"),
		}).toEqual({
			documentsSidebarIconSizing: true,
			documentsSidebarIconStroke: true,
			documentsStableIconSlots: true,
		})
	})
})
