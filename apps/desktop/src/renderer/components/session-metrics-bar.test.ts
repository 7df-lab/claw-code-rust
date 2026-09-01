import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const source = readFileSync(new URL("./session-metrics-bar.tsx", import.meta.url), "utf8")
const agentDetailSource = readFileSync(new URL("./agent-detail.tsx", import.meta.url), "utf8")
const chatViewSource = readFileSync(new URL("./chat/chat-view.tsx", import.meta.url), "utf8")

describe("SessionMetricsBar top timer wiring", () => {
	test("uses current chat turns for the inline timer instead of cumulative session work time", () => {
		expect({
			acceptsTurnsProp: source.includes("turns: ChatTurn[]"),
			acceptsIsWorkingProp: source.includes("isWorking: boolean"),
			usesLatestTurnTimer: source.includes("computeLatestTurnTimerSplit(turns"),
			omitsCompletedSessionWorkTime: !source.includes("completedMs={metrics.completedWorkTimeMs}"),
			exportsOverviewButton: source.includes("export function SessionMetricsOverviewButton"),
			headerUsesContextUsageButton: agentDetailSource.includes("<ContextUsageButton"),
		}).toEqual({
			acceptsTurnsProp: true,
			acceptsIsWorkingProp: true,
			usesLatestTurnTimer: true,
			omitsCompletedSessionWorkTime: true,
			exportsOverviewButton: true,
			headerUsesContextUsageButton: true,
		})
	})

	test("session header keeps Open in and ends with the three transcript controls", () => {
		const openInIndex = agentDetailSource.indexOf("<OpenInButton")
		const contextUsageIndex = agentDetailSource.indexOf("<ContextUsageButton")
		const terminalIndex = agentDetailSource.indexOf("<TerminalToggleButton")
		const changesIndex = agentDetailSource.indexOf("<ChangesPanelToggleButton")

		expect({
			keepsOpenInButton: openInIndex !== -1,
			removesCloseSessionIcon: !agentDetailSource.includes("XIcon"),
			exposesChangesPanelButton:
				agentDetailSource.includes("function ChangesPanelToggleButton") &&
				agentDetailSource.includes("onToggleReviewPanel"),
			exposesTerminalButton: agentDetailSource.includes("function TerminalToggleButton"),
			rightControlOrder:
				openInIndex !== -1 &&
				contextUsageIndex !== -1 &&
				terminalIndex !== -1 &&
				changesIndex !== -1 &&
				openInIndex < contextUsageIndex &&
				contextUsageIndex < terminalIndex &&
				terminalIndex < changesIndex,
		}).toEqual({
			keepsOpenInButton: true,
			removesCloseSessionIcon: true,
			exposesChangesPanelButton: true,
			exposesTerminalButton: true,
			rightControlOrder: true,
		})
	})

	test("session header context usage button shows occupancy breakdown", () => {
		const contextUsageSource = readFileSync(
			new URL("./context-usage-button.tsx", import.meta.url),
			"utf8",
		)
		expect({
			headerUsesContextUsageButton: agentDetailSource.includes("<ContextUsageButton"),
			replacesOverviewButtonInHeader: !agentDetailSource.includes("<SessionMetricsOverviewButton"),
			opensPromptBreakdown: contextUsageSource.includes("Prompt breakdown"),
			usesOccupancyCategories: contextUsageSource.includes("occupancyCategoryRows"),
			alignsToConversationSurface:
				contextUsageSource.includes("data-conversation-surface=") &&
				chatViewSource.includes("data-conversation-surface={agent.sessionId}") &&
				contextUsageSource.includes("createPortal") &&
				contextUsageSource.includes("max-w-3xl"),
			sitsFlushToTop:
				contextUsageSource.includes("absolute inset-x-0 top-0") &&
				contextUsageSource.includes('CONTEXT_USAGE_GUTTER_CLASS = "px-6 sm:px-10 lg:px-12"') &&
				!contextUsageSource.includes("sm:pt-8"),
			matchesAppPopoverChrome:
				contextUsageSource.includes("bg-popover") &&
				contextUsageSource.includes("ring-foreground/10") &&
				contextUsageSource.includes("rounded-md") &&
				contextUsageSource.includes("size-1.5 shrink-0 rounded-full") &&
				!contextUsageSource.includes("% Full") &&
				!contextUsageSource.includes("rounded-[3px]"),
			hasCloseButton:
				contextUsageSource.includes('aria-label="Close"') &&
				contextUsageSource.includes("XIcon"),
			replacesAnchoredPopover:
				!contextUsageSource.includes("PopoverContent") &&
				!contextUsageSource.includes("w-64"),
			doesNotReadBeforeResume: !contextUsageSource.includes("context.usage.read"),
			usesDefaultColor: contextUsageSource.includes("text-muted-foreground"),
			doesNotColorByUsage: !contextUsageSource.includes('percent >= 90 ? "text-red-400"') &&
				!contextUsageSource.includes('percent >= 70 ? "text-yellow-400"'),
		}).toEqual({
			headerUsesContextUsageButton: true,
			replacesOverviewButtonInHeader: true,
			opensPromptBreakdown: true,
			usesOccupancyCategories: true,
			alignsToConversationSurface: true,
			sitsFlushToTop: true,
			matchesAppPopoverChrome: true,
			hasCloseButton: true,
			replacesAnchoredPopover: true,
			doesNotReadBeforeResume: true,
			usesDefaultColor: true,
			doesNotColorByUsage: true,
		})
	})

	test("session header panel toggles use panel icons", () => {
		expect({
			usesBottomPanelIcon: agentDetailSource.includes("BottomPanelIcon"),
			usesRightPanelIcon: agentDetailSource.includes("RightPanelIcon"),
			replacesLucideTerminalIcon: !agentDetailSource.includes("TerminalIcon"),
			replacesLucidePanelRightIcon: !agentDetailSource.includes("PanelRightIcon"),
		}).toEqual({
			usesBottomPanelIcon: true,
			usesRightPanelIcon: true,
			replacesLucideTerminalIcon: true,
			replacesLucidePanelRightIcon: true,
		})
	})
})
