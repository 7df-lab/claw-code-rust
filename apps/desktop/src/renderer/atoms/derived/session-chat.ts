import type { Message, Part } from "../../lib/types"
import { getStreamingPartsForSession } from "../streaming"

const DEVO_ITEM_KIND_META = "devo/itemKind"
const DEVO_RESEARCH_ARTIFACT_TITLE_META = "devo/researchArtifactTitle"

// ============================================================
// Types — wrappers around SDK Message + Part
// ============================================================

/** A message with its associated parts */
export interface ChatMessageEntry {
	info: Message
	parts: Part[]
}

/**
 * A "turn" groups a user message with its assistant responses.
 */
export interface ChatTurn {
	id: string
	/** Protocol turn id when available (`Message.turnID` / `devo/turnId`). */
	turnId?: string
	userMessage: ChatMessageEntry
	assistantMessages: ChatMessageEntry[]
}

function messageProtocolTurnId(entry: ChatMessageEntry): string | undefined {
	const info = entry.info as {
		turnID?: unknown
		turnId?: unknown
		metadata?: Record<string, unknown>
	}
	const direct = info.turnID ?? info.turnId
	if (typeof direct === "string" && direct.length > 0) return direct
	const metaTurnId = info.metadata?.["devo/turnId"]
	return typeof metaTurnId === "string" && metaTurnId.length > 0 ? metaTurnId : undefined
}

function protocolTurnIdForTurn(
	userMessage: ChatMessageEntry,
	assistantMessages: ChatMessageEntry[],
): string | undefined {
	return (
		messageProtocolTurnId(userMessage) ??
		assistantMessages.map(messageProtocolTurnId).find((turnId): turnId is string => Boolean(turnId))
	)
}

// ============================================================
// Turn grouping with structural sharing
// ============================================================

function toolOutputFingerprint(part: {
	state: {
		status: string
		output?: string
		error?: string
		metadata?: { output?: unknown }
		raw?: unknown
	}
}): number {
	if (part.state.status === "completed") return part.state.output.length
	if (part.state.status === "error") return part.state.error.length
	if (part.state.status === "running") {
		const output = part.state.metadata?.output
		return typeof output === "string" ? output.length : 0
	}
	if (part.state.status === "pending") {
		return typeof part.state.raw === "string" ? part.state.raw.length : 0
	}
	return 0
}

function messageFingerprint(entry: ChatMessageEntry): string {
	const lastPart = entry.parts.at(-1)
	const completed = entry.info.role === "assistant" ? (entry.info.time.completed ?? 0) : 0
	let textLen = 0
	const toolSegments: string[] = []
	const textMetadataSegments: string[] = []
	for (const part of entry.parts) {
		if (part.type === "text" || part.type === "reasoning") {
			textLen += part.text.length
			if (part.type === "text") {
				const metadata = (part as { metadata?: Record<string, unknown> }).metadata
				if (metadata?.[DEVO_ITEM_KIND_META] === "research_artifact") {
					textMetadataSegments.push(
						`${part.id}:${metadata[DEVO_ITEM_KIND_META]}:${metadata[DEVO_RESEARCH_ARTIFACT_TITLE_META] ?? ""}`,
					)
				}
				if (
					metadata?.[DEVO_ITEM_KIND_META] === "context_compaction" ||
					metadata?.[DEVO_ITEM_KIND_META] === "proposed_plan" ||
					metadata?.[DEVO_ITEM_KIND_META] === "plan"
				) {
					textMetadataSegments.push(
						`${part.id}:${metadata[DEVO_ITEM_KIND_META]}:${metadata["devo/compactionStatus"] ?? ""}:${part.text.length}`,
					)
				}
			}
		} else if (part.type === "tool") {
			toolSegments.push(`${part.id}:${part.state.status}:${toolOutputFingerprint(part)}`)
		}
	}
	return `${entry.info.id}:${completed}:${entry.parts.length}:${lastPart?.id ?? ""}:${textLen}:${textMetadataSegments.join(",")}:${toolSegments.join(",")}`
}

function turnFingerprint(turn: ChatTurn): string {
	const assistantFps = turn.assistantMessages.map(messageFingerprint).join("|")
	return `${turn.turnId ?? ""}:${messageFingerprint(turn.userMessage)}>${assistantFps}`
}

export function groupIntoTurns(entries: ChatMessageEntry[], prevTurns: ChatTurn[]): ChatTurn[] {
	const prevMap = new Map<string, ChatTurn>()
	for (const t of prevTurns) {
		prevMap.set(turnFingerprint(t), t)
	}

	const turns: ChatTurn[] = []
	let currentUser: ChatMessageEntry | null = null
	let currentAssistantMessages: ChatMessageEntry[] = []

	const flushTurn = () => {
		if (!currentUser) return
		const newTurn: ChatTurn = {
			id: currentUser.info.id,
			turnId: protocolTurnIdForTurn(currentUser, currentAssistantMessages),
			userMessage: currentUser,
			assistantMessages: currentAssistantMessages,
		}

		const fp = turnFingerprint(newTurn)
		const prevTurn = prevMap.get(fp)
		turns.push(prevTurn ?? newTurn)
	}

	for (const entry of entries) {
		if (entry.info.role === "user") {
			flushTurn()
			currentUser = entry
			currentAssistantMessages = []
			continue
		}
		if (
			entry.info.role === "assistant" &&
			currentUser &&
			(!entry.info.parentID || entry.info.parentID === currentUser.info.id)
		) {
			currentAssistantMessages.push(entry)
		}
	}
	flushTurn()

	return turns
}

/**
 * Merges streaming buffer overlays with base store parts for a given session.
 * This is used from useSessionChat to build ChatMessageEntry[] with streaming data merged.
 *
 * Now scoped by sessionId so only that session's streaming buffer is read,
 * and uses the per-session streaming version for dependency tracking.
 */
export function mergeSessionParts(
	sessionId: string,
	messages: Message[],
	getParts: (messageId: string) => Part[],
	_streamingVersion: number,
): ChatMessageEntry[] {
	// _streamingVersion is passed to establish dependency tracking in the calling useMemo
	const streaming = getStreamingPartsForSession(sessionId)

	return messages.map((msg) => {
		const baseParts = getParts(msg.id)

		const overrides = streaming[msg.id]
		if (!overrides) {
			return { info: msg, parts: baseParts }
		}

		// Overlay streaming parts on top of the base parts
		const overlaid = baseParts.map((part) => overrides[part.id] ?? part)

		// Include streaming parts that don't exist in the base store yet
		const baseIds = new Set(baseParts.map((p) => p.id))
		for (const partId in overrides) {
			if (!baseIds.has(partId)) {
				overlaid.push(overrides[partId])
			}
		}

		return { info: msg, parts: overlaid }
	})
}
