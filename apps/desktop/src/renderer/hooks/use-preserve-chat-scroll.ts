import { useStickToBottomContext } from "@devo/ui/components/ai-elements/conversation"
import { useCallback } from "react"

/**
 * Run `action` without the conversation stick-to-bottom layer reacting to the
 * resulting layout shift (e.g. expanding Thought / Added / Edited rows).
 */
export function usePreserveChatScroll() {
	const { scrollRef, stopScroll } = useStickToBottomContext()

	return useCallback(
		(action: () => void) => {
			const scrollEl = scrollRef.current
			const savedTop = scrollEl?.scrollTop ?? null

			stopScroll()
			action()

			if (scrollEl == null || savedTop == null) return

			const restore = () => {
				scrollEl.scrollTop = savedTop
			}

			restore()
			requestAnimationFrame(() => {
				restore()
				requestAnimationFrame(() => {
					restore()
					window.setTimeout(() => {
						restore()
						stopScroll()
					}, 32)
				})
			})
		},
		[scrollRef, stopScroll],
	)
}
