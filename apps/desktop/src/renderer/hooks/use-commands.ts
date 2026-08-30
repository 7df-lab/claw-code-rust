import { useAtomValue } from "jotai"
import { useCallback, useMemo } from "react"
import { messagesFamily } from "../atoms/messages"
import { partsFamily, partStorageKey } from "../atoms/parts"
import { sessionFamily } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { formatShortcut } from "../lib/shortcut-display"
import type { Session, TextPart } from "../lib/types"
import { getProjectClient } from "../services/connection-manager"
import { useServerCommands } from "./use-devo-data"

// ============================================================
// Types
// ============================================================

export interface AppCommand {
	name: string
	label: string
	description: string
	enabled: boolean
	shortcut?: string
	execute: () => Promise<void>
	source: "client" | "server"
}

// ============================================================
// useSessionRevert — undo/redo logic
// ============================================================

function findUndoTarget(sessionId: string, revertMessageId?: string): string | null {
	const messages = appStore.get(messagesFamily(sessionId))
	if (!messages || messages.length === 0) return null

	let lastUserMsgId: string | null = null
	for (let i = messages.length - 1; i >= 0; i--) {
		const msg = messages[i]
		if (msg.role !== "user") continue
		if (revertMessageId && msg.id >= revertMessageId) continue
		lastUserMsgId = msg.id
		break
	}
	return lastUserMsgId
}

function findRedoTarget(sessionId: string, revertMessageId: string): string | null {
	const messages = appStore.get(messagesFamily(sessionId))
	if (!messages) return null

	let foundRevertPoint = false
	for (const msg of messages) {
		if (msg.id === revertMessageId) {
			foundRevertPoint = true
			continue
		}
		if (foundRevertPoint && msg.role === "user") {
			return msg.id
		}
	}
	return null
}

function getUserMessageText(sessionId: string, messageId: string): string {
	const parts = appStore.get(partsFamily(partStorageKey(sessionId, messageId)))
	if (!parts) return ""
	return parts
		.filter((p): p is TextPart => p.type === "text" && !("synthetic" in p && p.synthetic))
		.map((p) => p.text)
		.join("\n")
}

export interface UseSessionRevertResult {
	isReverted: boolean
	revertInfo: Session["revert"] | undefined
	canUndo: boolean
	canRedo: boolean
	undo: () => Promise<string | undefined>
	redo: () => Promise<void>
}

export function useSessionRevert(
	directory: string | null,
	sessionId: string | null,
): UseSessionRevertResult {
	const entry = useAtomValue(sessionFamily(sessionId ?? ""))
	const session = entry?.session
	const messages = useAtomValue(messagesFamily(sessionId ?? ""))

	const isReverted = !!session?.revert
	const revertInfo = session?.revert

	const canUndo = useMemo(() => {
		if (!directory || !sessionId || !messages || messages.length === 0) return false
		const target = findUndoTarget(sessionId, revertInfo?.messageID)
		return target !== null
	}, [directory, sessionId, messages, revertInfo])

	const canRedo = isReverted

	const undo = useCallback(async (): Promise<string | undefined> => {
		if (!directory || !sessionId) return undefined
		const client = getProjectClient(directory)
		if (!client) return undefined

		const sessionEntry = appStore.get(sessionFamily(sessionId))
		if (sessionEntry?.status?.type === "busy") {
			await client.session.abort({ sessionID: sessionId })
		}

		const targetId = findUndoTarget(sessionId, revertInfo?.messageID)
		if (!targetId) return undefined

		const userText = getUserMessageText(sessionId, targetId)
		await client.session.revert({ sessionID: sessionId, messageID: targetId })
		return userText
	}, [directory, sessionId, revertInfo])

	const redo = useCallback(async () => {
		if (!directory || !sessionId || !revertInfo) return
		const client = getProjectClient(directory)
		if (!client) return

		const nextTarget = findRedoTarget(sessionId, revertInfo.messageID)
		if (nextTarget) {
			await client.session.revert({ sessionID: sessionId, messageID: nextTarget })
		} else {
			await client.session.unrevert({ sessionID: sessionId })
		}
	}, [directory, sessionId, revertInfo])

	return { isReverted, revertInfo, canUndo, canRedo, undo, redo }
}

// ============================================================
// useCommands — unified command registry
// ============================================================

export function useCommands(
	directory: string | null,
	sessionId: string | null,
	options?: {
		onUndoTextRestore?: (text: string) => void
	},
): AppCommand[] {
	const { canUndo, canRedo, undo, redo } = useSessionRevert(directory, sessionId)
	const serverCommands = useServerCommands(directory)
	const entry = useAtomValue(sessionFamily(sessionId ?? ""))
	const sessionStatus = entry?.status
	const isIdle = sessionStatus?.type === "idle" || !sessionStatus
	const undoShortcut = formatShortcut(["mod", "Z"])
	const redoShortcut = formatShortcut(["shift", "mod", "Z"])

	const clientCommands = useMemo<AppCommand[]>(() => {
		const cmds: AppCommand[] = []

		cmds.push({
			name: "undo",
			label: "Undo",
			description: "Undo the last turn and restore file changes",
			enabled: canUndo,
			shortcut: undoShortcut,
			source: "client",
			execute: async () => {
				const text = await undo()
				if (text && options?.onUndoTextRestore) {
					options.onUndoTextRestore(text)
				}
			},
		})

		cmds.push({
			name: "redo",
			label: "Redo",
			description: "Restore previously undone messages",
			enabled: canRedo,
			shortcut: redoShortcut,
			source: "client",
			execute: async () => {
				await redo()
			},
		})

		cmds.push({
			name: "compact",
			label: "Compact",
			description: "Summarize the conversation to save context",
			enabled: !!directory && !!sessionId && isIdle,
			source: "client",
			execute: async () => {
				if (!directory || !sessionId) return
				const client = getProjectClient(directory)
				if (!client) return
				await client.session.summarize({ sessionID: sessionId })
			},
		})

		return cmds
	}, [
		canUndo,
		canRedo,
		undo,
		redo,
		undoShortcut,
		redoShortcut,
		directory,
		sessionId,
		isIdle,
		options?.onUndoTextRestore,
		options,
	])

	const allCommands = useMemo<AppCommand[]>(() => {
		const serverCmds: AppCommand[] = serverCommands.map((cmd) => ({
			name: cmd.name,
			label: cmd.name.charAt(0).toUpperCase() + cmd.name.slice(1),
			description: cmd.description ?? `Run /${cmd.name}`,
			enabled: !!directory && !!sessionId && isIdle,
			source: "server" as const,
			execute: async () => {
				if (!directory || !sessionId) return
				const client = getProjectClient(directory)
				if (!client) return
				await client.session.command({
					sessionID: sessionId,
					command: cmd.name,
					arguments: "",
				})
			},
		}))
		return [...clientCommands, ...serverCmds]
	}, [clientCommands, serverCommands, directory, sessionId, isIdle])

	return allCommands
}
