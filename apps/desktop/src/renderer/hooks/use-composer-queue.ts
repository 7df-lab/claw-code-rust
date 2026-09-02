import { useAtomValue } from "jotai"
import { useCallback, useEffect, useMemo, useState } from "react"
import { sessionActiveTurnFamily, sessionQueueFamily } from "../atoms/queue"
import { createLogger } from "../lib/logger"
import {
	countQueueFileParts,
	queueEntryText,
	type QueueWireEntry,
} from "../lib/queue-helpers"
import { getProjectClient } from "../services/connection-manager"
import type {
	ComposerQueueItem,
	ComposerQueueItemStatus,
} from "../components/chat/composer-status-stack"

const log = createLogger("use-composer-queue")

type QueueAction = "steering" | "removing" | "submitting"

function wireEntryToComposerItem(
	entry: QueueWireEntry,
	action: QueueAction | null,
	error?: string,
): ComposerQueueItem {
	return {
		id: entry.queueItemId,
		text: queueEntryText(entry),
		status: action ?? "queued",
		queuedInputId: entry.queueItemId,
		fileCount: countQueueFileParts(entry) || undefined,
		createdAtMs: entry.enqueuedAt ? Date.parse(entry.enqueuedAt) : undefined,
		error,
	}
}

export function useComposerQueue(sessionId: string, directory: string | null) {
	const entries = useAtomValue(sessionQueueFamily(sessionId))
	const activeTurnId = useAtomValue(sessionActiveTurnFamily(sessionId))
	const [pendingActions, setPendingActions] = useState<Map<string, QueueAction>>(new Map())
	const [actionErrors, setActionErrors] = useState<Map<string, string>>(new Map())
	const [draggingId, setDraggingId] = useState<string | null>(null)

	const refreshQueue = useCallback(async () => {
		if (!directory) return
		const client = getProjectClient(directory)
		if (!client?.session?.queue?.list) return
		try {
			await client.session.queue.list({ sessionID: sessionId })
		} catch (err) {
			log.error("queue.list failed", { sessionId }, err)
		}
	}, [directory, sessionId])

	useEffect(() => {
		void refreshQueue()
	}, [refreshQueue])

	useEffect(() => {
		setPendingActions(new Map())
		setActionErrors(new Map())
	}, [sessionId])

	const queueItems = useMemo(() => {
		return entries.map((entry) =>
			wireEntryToComposerItem(
				entry,
				pendingActions.get(entry.queueItemId) ?? null,
				actionErrors.get(entry.queueItemId),
			),
		)
	}, [actionErrors, entries, pendingActions])

	const setPending = useCallback((queueItemId: string, action: QueueAction | null) => {
		setPendingActions((current) => {
			const next = new Map(current)
			if (action) next.set(queueItemId, action)
			else next.delete(queueItemId)
			return next
		})
	}, [])

	const setItemError = useCallback((queueItemId: string, message: string | null) => {
		setActionErrors((current) => {
			const next = new Map(current)
			if (message) next.set(queueItemId, message)
			else next.delete(queueItemId)
			return next
		})
	}, [])

	const steerQueueItem = useCallback(
		async (item: ComposerQueueItem) => {
			if (!directory || !clientSupportsQueue(directory)) return
			const client = getProjectClient(directory)
			if (!client?.session?.queue?.steer) return
			setPending(item.id, "steering")
			setItemError(item.id, null)
			try {
				await client.session.queue.steer({
					sessionID: sessionId,
					queueItemId: item.id,
				})
			} catch (err) {
				const message = err instanceof Error ? err.message : "Steer failed"
				setItemError(item.id, message)
				log.error("queue.steer failed", { sessionId, queueItemId: item.id }, err)
			} finally {
				setPending(item.id, null)
			}
		},
		[directory, sessionId, setItemError, setPending],
	)

	const removeQueueItem = useCallback(
		async (item: ComposerQueueItem) => {
			if (!directory || !clientSupportsQueue(directory)) return
			const client = getProjectClient(directory)
			if (!client?.session?.queue?.remove) return
			setPending(item.id, "removing")
			setItemError(item.id, null)
			try {
				await client.session.queue.remove({
					sessionID: sessionId,
					queueItemId: item.id,
				})
			} catch (err) {
				const message = err instanceof Error ? err.message : "Remove failed"
				setItemError(item.id, message)
				log.error("queue.remove failed", { sessionId, queueItemId: item.id }, err)
				throw err
			} finally {
				setPending(item.id, null)
			}
		},
		[directory, sessionId, setItemError, setPending],
	)

	const editQueueItem = useCallback(
		async (item: ComposerQueueItem) => {
			if ((item.fileCount ?? 0) > 0) return null
			await removeQueueItem(item)
			return item.text
		},
		[removeQueueItem],
	)

	const reorderQueueItem = useCallback(
		async (fromIndex: number, toIndex: number) => {
			if (!directory || fromIndex === toIndex) return
			const client = getProjectClient(directory)
			if (!client?.session?.queue?.update) return
			const item = entries[fromIndex]
			if (!item) return
			try {
				await client.session.queue.update({
					sessionID: sessionId,
					queueItemId: item.queueItemId,
					position: toIndex,
				})
			} catch (err) {
				log.error("queue.reorder failed", { sessionId, fromIndex, toIndex }, err)
				void refreshQueue()
			}
		},
		[directory, entries, refreshQueue, sessionId],
	)

	return {
		queueItems,
		activeTurnId,
		draggingId,
		setDraggingId,
		steerQueueItem,
		removeQueueItem,
		editQueueItem,
		reorderQueueItem,
	}
}

function clientSupportsQueue(directory: string): boolean {
	const client = getProjectClient(directory)
	return !!client?.session?.queue
}

export function mapQueueItemStatus(status: ComposerQueueItemStatus): QueueAction | null {
	switch (status) {
		case "steering":
			return "steering"
		case "removing":
			return "removing"
		case "submitting":
			return "submitting"
		default:
			return null
	}
}
