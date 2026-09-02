import { createLogger } from "../../lib/logger"
import { queryClient } from "../../lib/query-client"
import type { Event } from "../../lib/types"
import { compactionStatusFamily } from "../compaction"
import { serverConnectedAtom } from "../connection"
import { discoveryAtom } from "../discovery"
import { removeMessageAtom, upsertMessageAtom } from "../messages"
import { applyPartDeltaAtom, removePartAtom, upsertPartAtom } from "../parts"
import {
	addPermissionAtom,
	addQuestionAtom,
	removePermissionAtom,
	removeQuestionAtom,
	removeSessionAtom,
	setProviderRetryStatusAtom,
	setSessionErrorAtom,
	setSessionStatusAtom,
	upsertSessionAtom,
} from "../sessions"
import { setSessionActiveTurnAtom, setSessionQueueAtom } from "../queue"
import { sessionNativeFamily } from "../session-native"
import { appStore } from "../store"
import { isStreamingField, streamingVersionFamily } from "../streaming"
import { todosFamily } from "../todos"
import { setSessionDiffAtom } from "../ui"
import { applyWorkspaceChangesUpdatedAtom } from "../workspace-changes"

const log = createLogger("event-processor")

/**
 * Invalidate all Devo data queries for a specific directory.
 * Called when an instance is disposed so the UI re-fetches config, agents, providers, etc.
 */
function invalidateDirectoryQueries(directory: string): void {
	log.info("Invalidating queries for disposed instance", { directory })
	for (const key of ["config", "providers", "agents", "commands", "vcs"]) {
		queryClient.invalidateQueries({ queryKey: [key, directory] })
	}
}

/**
 * Invalidate all Devo data queries across all directories.
 * Called when a global dispose event occurs (e.g. global config change).
 */
function invalidateAllQueries(): void {
	log.info("Invalidating all Devo queries (global dispose)")
	for (const key of ["config", "providers", "agents", "commands", "vcs"]) {
		queryClient.invalidateQueries({ queryKey: [key] })
	}
}

/**
 * Central Native event dispatcher.
 * A standalone function that writes to Jotai atoms via the store API.
 * Called by the event batcher in connection-manager.
 */
export function processEvent(event: Event): void {
	const { set } = appStore

	switch (event.type) {
		case "server.connected":
			set(serverConnectedAtom, true)
			break

		case "server.instance.disposed": {
			const directory = event.properties.directory
			if (directory) {
				invalidateDirectoryQueries(directory)
			}
			break
		}

		case "global.disposed":
			invalidateAllQueries()
			break

		case "project.updated": {
			const project = event.properties
			if (project.id && project.worktree) {
				const current = appStore.get(discoveryAtom)
				const existing = current.projects.findIndex((p) => p.id === project.id)
				const nextProjects =
					existing >= 0
						? current.projects.map((p, i) => (i === existing ? project : p))
						: [...current.projects, project]
				set(discoveryAtom, { ...current, projects: nextProjects })
			}
			break
		}

		case "session.created": {
			const info = event.properties.info
			set(upsertSessionAtom, { session: info, directory: info.directory ?? "" })
			break
		}

		case "session.updated": {
			const info = event.properties.info
			set(upsertSessionAtom, { session: info, directory: info.directory ?? "" })
			break
		}

		case "session.deleted":
			set(removeSessionAtom, event.properties.info.id)
			break

		case "turn.provider_retry_status": {
			const properties = event.properties
			const sessionId = properties.sessionID ?? properties.session_id
			const turnId = properties.turnID ?? properties.turn_id
			if (sessionId && turnId) {
				const phase = String(properties.phase ?? "")
				set(setProviderRetryStatusAtom, {
					sessionId,
					status:
						phase === "resumed"
							? undefined
							: {
								turnId,
								attempt: Number(properties.attempt ?? 0),
								backoffMs: Number(properties.backoffMs ?? properties.backoff_ms ?? 0),
								provider: String(properties.provider ?? ""),
								model: String(properties.model ?? ""),
								phase,
								message: String(properties.message ?? ""),
							},
				})
			}
			break
		}

		case "session.status":
			set(setSessionStatusAtom, {
				sessionId: event.properties.sessionID,
				status: event.properties.status,
			})
			// Clear error when session starts working again
			if (event.properties.status.type !== "idle") {
				set(setSessionErrorAtom, {
					sessionId: event.properties.sessionID,
					error: undefined,
				})
			}
			break

		case "session.activeTurn":
			set(setSessionActiveTurnAtom, {
				sessionId: event.properties.sessionID,
				turnId: event.properties.turnID ?? null,
			})
			break

		case "session.queue.updated":
			set(setSessionQueueAtom, {
				sessionId: event.properties.sessionID,
				entries: event.properties.entries ?? [],
			})
			break

		case "session.error": {
			const { sessionID, error } = event.properties
			if (sessionID && error) {
				set(setSessionErrorAtom, {
					sessionId: sessionID,
					error: { name: error.name, data: error.data },
				})
			}
			break
		}

		case "session.compaction.started":
		case "session/compaction/started": {
			const sessionID = event.properties.sessionID ?? event.properties.session_id
			if (sessionID) {
				set(compactionStatusFamily(sessionID), "started")
			}
			break
		}

		case "session.compaction.completed":
		case "session/compaction/completed": {
			const sessionID = event.properties.sessionID ?? event.properties.session_id
			if (sessionID) {
				set(compactionStatusFamily(sessionID), "completed")
			}
			break
		}

		case "session.compaction.failed":
		case "session/compaction/failed": {
			const sessionID = event.properties.sessionID ?? event.properties.session_id
			if (sessionID) {
				set(compactionStatusFamily(sessionID), null)
			}
			break
		}

		case "permission.asked":
			set(addPermissionAtom, {
				sessionId: event.properties.sessionID,
				permission: event.properties,
			})
			break

		case "permission.replied":
			set(removePermissionAtom, {
				sessionId: event.properties.sessionID,
				permissionId: event.properties.requestID,
			})
			break

		case "question.asked":
			set(addQuestionAtom, {
				sessionId: event.properties.sessionID,
				question: event.properties,
			})
			break

		case "question.replied":
			set(removeQuestionAtom, {
				sessionId: event.properties.sessionID,
				requestId: event.properties.requestID,
			})
			break

		case "question.rejected":
			set(removeQuestionAtom, {
				sessionId: event.properties.sessionID,
				requestId: event.properties.requestID,
			})
			break

		case "message.updated":
			set(upsertMessageAtom, event.properties.info)
			break

		case "message.removed":
			set(removeMessageAtom, {
				sessionId: event.properties.sessionID,
				messageId: event.properties.messageID,
			})
			break

		case "message.part.updated": {
			const part = event.properties.part
			set(upsertPartAtom, part)
			// useSessionChat reads partsFamily imperatively through appStore.get,
			// so every visible part update must bump the per-session version.
			// Streaming text/reasoning parts may also be present in the streaming
			// overlay, but the main-store write still needs to invalidate renders
			// when it is the event that reaches the UI after a tool call.
			set(streamingVersionFamily(part.sessionID), (v) => v + 1)
			break
		}

		case "message.part.delta": {
			const { messageID, partID, field, delta, sessionID } = event.properties
			set(applyPartDeltaAtom, { sessionId: sessionID, messageId: messageID, partId: partID, field, delta })
			// Non-streaming field deltas (e.g. tool input) bypass the streaming
			// buffer and land directly in partsFamily. Bump the version so the
			// UI re-renders to show the updated content.
			if (!isStreamingField(field)) {
				set(streamingVersionFamily(sessionID), (v) => v + 1)
			}
			break
		}

		case "message.part.removed": {
			const { messageID, partID, sessionID } = event.properties
			set(removePartAtom, { sessionId: sessionID, messageId: messageID, partId: partID })
			// Part removal changes the visible part list, so notify the session.
			set(streamingVersionFamily(sessionID), (v) => v + 1)
			break
		}

		case "todo.updated":
			set(todosFamily(event.properties.sessionID), event.properties.todos)
			break

		case "session.commands.updated": {
			const sessionID = event.properties.sessionID
			if (!sessionID) break
			const current = appStore.get(sessionNativeFamily(sessionID))
			set(sessionNativeFamily(sessionID), {
				...current,
				commands: event.properties.commands ?? [],
			})
			break
		}

		case "session.config.updated": {
			const sessionID = event.properties.sessionID
			if (!sessionID) break
			const current = appStore.get(sessionNativeFamily(sessionID))
			set(sessionNativeFamily(sessionID), {
				...current,
				configOptions: event.properties.configOptions ?? [],
			})
			break
		}

		case "session.mode.updated": {
			const sessionID = event.properties.sessionID
			if (!sessionID) break
			const current = appStore.get(sessionNativeFamily(sessionID))
			set(sessionNativeFamily(sessionID), {
				...current,
				modeID: event.properties.modeID,
			})
			break
		}

		case "session.usage.updated": {
			const sessionID = event.properties.sessionID
			if (!sessionID) break
			const current = appStore.get(sessionNativeFamily(sessionID))
			set(sessionNativeFamily(sessionID), {
				...current,
				usage: {
					used: event.properties.used,
					size: event.properties.size,
					cost: event.properties.cost,
				},
			})
			break
		}

		case "context.usage.updated": {
			const sessionID = event.properties.sessionID
			if (!sessionID) break
			const current = appStore.get(sessionNativeFamily(sessionID))
			set(sessionNativeFamily(sessionID), {
				...current,
				occupancy: event.properties.occupancy,
			})
			break
		}

		case "session.diff": {
			const { sessionID, diff } = event.properties as {
				sessionID: string
				diff: import("../../lib/types").FileDiff[]
			}
			if (sessionID && diff) {
				set(setSessionDiffAtom, { sessionId: sessionID, diffs: diff })
			}
			break
		}

		case "workspace.changes.updated":
			set(applyWorkspaceChangesUpdatedAtom, event.properties)
			break

		// --- Worktree lifecycle events (from Devo experimental API) ---

		case "worktree.ready":
			log.info("Worktree ready", {
				name: event.properties.name,
				branch: event.properties.branch,
			})
			break

		case "worktree.failed":
			log.warn("Worktree creation failed", {
				message: event.properties.message,
			})
			break
	}
}
