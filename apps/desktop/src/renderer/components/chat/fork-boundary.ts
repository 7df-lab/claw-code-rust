import type { ChatTurn } from "../../atoms/derived/session-chat"

/** Index of the last inherited turn; render the fork marker immediately after it. */
export function forkBoundaryAfterTurnIndex(
	turns: ChatTurn[],
	forkFromId: string | undefined,
	atTurnId: string | undefined,
	forkSessionCreatedAt: number,
): number {
	if (!forkFromId || turns.length === 0) return -1

	if (atTurnId) {
		const turnIndex = turns.findIndex((turn) => turn.turnId === atTurnId)
		return turnIndex >= 0 ? turnIndex : -1
	}

	let lastInherited = -1
	for (let index = 0; index < turns.length; index++) {
		const created = turns[index].userMessage.info.time?.created
		if (typeof created === "number" && created <= forkSessionCreatedAt) {
			lastInherited = index
			continue
		}
		break
	}
	return lastInherited
}
