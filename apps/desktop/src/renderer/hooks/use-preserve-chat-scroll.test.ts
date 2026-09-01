import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const conversationSource = readFileSync(
	new URL("../../../packages/ui/src/components/ai-elements/conversation.tsx", import.meta.url),
	"utf8",
)
const transcriptDisclosureSource = readFileSync(
	new URL("../components/chat/transcript-disclosure.tsx", import.meta.url),
	"utf8",
)
const chatTurnSource = readFileSync(
	new URL("../components/chat/chat-turn.tsx", import.meta.url),
	"utf8",
)
const preserveScrollSource = readFileSync(
	new URL("./use-preserve-chat-scroll.ts", import.meta.url),
	"utf8",
)

describe("chat scroll preservation on transcript expand", () => {
	test("does not smooth-scroll the whole conversation on inline resizes", () => {
		expect({
			instantResize: conversationSource.includes('resize="instant"'),
			noSmoothResize: !conversationSource.includes('resize="smooth"'),
		}).toEqual({
			instantResize: true,
			noSmoothResize: true,
		})
	})

	test("preserves scroll position when transcript rows toggle", () => {
		expect({
			hook: preserveScrollSource.includes("export function usePreserveChatScroll"),
			stopsStickToBottom: preserveScrollSource.includes("stopScroll()"),
			restoresScrollTop: preserveScrollSource.includes("scrollEl.scrollTop = savedTop"),
			transcriptUsesHook: transcriptDisclosureSource.includes("usePreserveChatScroll"),
			completedProcessUsesHook: chatTurnSource.includes("usePreserveChatScroll"),
		}).toEqual({
			hook: true,
			stopsStickToBottom: true,
			restoresScrollTop: true,
			transcriptUsesHook: true,
			completedProcessUsesHook: true,
		})
	})
})
