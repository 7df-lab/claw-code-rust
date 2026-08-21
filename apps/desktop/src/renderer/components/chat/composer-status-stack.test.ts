import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const stackSource = readFileSync(new URL("./composer-status-stack.tsx", import.meta.url), "utf8")
const chatViewSource = readFileSync(new URL("./chat-view.tsx", import.meta.url), "utf8")
const chatTurnSource = readFileSync(new URL("./chat-turn.tsx", import.meta.url), "utf8")
const clientSource = readFileSync(
	new URL("../../../../packages/devo-ai-sdk/src/v2/client.ts", import.meta.url),
	"utf8",
)

describe("ComposerStatusStack", () => {
	test("renders active goal state in the composer-adjacent status area", () => {
		expect({
			component: stackSource.includes("export function ComposerStatusStack"),
			requirementComment: stackSource.includes("reuse this composer-adjacent strip"),
			activeLabel: stackSource.includes("Pursuing goal"),
			pausedLabel: stackSource.includes("Goal paused"),
			budgetLabel: stackSource.includes("Goal budget reached"),
			goalIcon: stackSource.includes("GoalIcon"),
			editAction: stackSource.includes("PencilIcon"),
			pauseAction: stackSource.includes("CirclePauseIcon"),
			resumeAction: stackSource.includes("CirclePlayIcon"),
			clearAction: stackSource.includes("Trash2Icon"),
			iconSizeMatchesDesktop: stackSource.includes("size-3.5") && stackSource.includes("stroke-[1.5]"),
			composerPlacement: chatViewSource.includes("<ComposerStatusStack"),
			mergedComposerShape: chatViewSource.includes("activeGoal && \"rounded-t-none"),
		}).toEqual({
			component: true,
			requirementComment: true,
			activeLabel: true,
			pausedLabel: true,
			budgetLabel: true,
			goalIcon: true,
			editAction: true,
			pauseAction: true,
			resumeAction: true,
			clearAction: true,
			iconSizeMatchesDesktop: true,
			composerPlacement: true,
			mergedComposerShape: true,
		})
	})

	test("connects the composer goal row to existing goal RPC methods", () => {
		expect({
			normalizesGoal: chatViewSource.includes("function normalizeComposerGoal"),
			loadsGoalStatus: chatViewSource.includes("client.goal.status"),
			pausesGoal: chatViewSource.includes("client.goal.pause"),
			resumesGoal: chatViewSource.includes("client.goal.resume"),
			clearsGoal: chatViewSource.includes("client.goal.clear"),
			editReusesGoalTrigger: chatViewSource.includes('setActiveTrigger("goal")'),
			refreshesAfterGoalPrompt: chatViewSource.includes('if (trigger === "goal")'),
			clientStatus: clientSource.includes('"session/goal/read"'),
			clientPause: clientSource.includes('"session/goal/pause"'),
			clientResume: clientSource.includes('"session/goal/resume"'),
			clientClear: clientSource.includes('"session/goal/clear"'),
			noLegacyGoalMethods: ![
				"goal/create",
				"goal/set",
				"goal/status",
				"goal/pause",
				"goal/resume",
				"goal/complete",
				"goal/clear",
			].some((method) => clientSource.includes(`"${method}"`)),
		}).toEqual({
			normalizesGoal: true,
			loadsGoalStatus: true,
			pausesGoal: true,
			resumesGoal: true,
			clearsGoal: true,
			editReusesGoalTrigger: true,
			refreshesAfterGoalPrompt: true,
			clientStatus: true,
			clientPause: true,
			clientResume: true,
			clientClear: true,
			noLegacyGoalMethods: true,
		})
	})

	test("keeps queued follow-up controls out of transcript turns", () => {
		expect({
			requirementComment: chatTurnSource.includes("queue state belongs in the composer status stack"),
			noSendNowProp: !chatTurnSource.includes("onSendNow"),
			noSendNowLabel: !chatTurnSource.includes("Send now"),
			noQueuedInference: !chatTurnSource.includes("isQueued = isWorking"),
			noQueueLabel: !chatTurnSource.includes(">Queued<"),
		}).toEqual({
			requirementComment: true,
			noSendNowProp: true,
			noSendNowLabel: true,
			noQueuedInference: true,
			noQueueLabel: true,
		})
	})
})
