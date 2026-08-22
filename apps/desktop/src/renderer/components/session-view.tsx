/**
 * Reusable session view component.
 *
 * Renders the full chat UI (AgentDetail with ChatView, prompt input, app bar
 * integration, undo/redo, permissions, etc.) for any given sessionId.
 *
 * This is the extracted "controller" logic that was previously inlined in
 * SessionRoute. Both SessionRoute (for route-driven sessions) and
 * AutomationRunDetail (for automation sessions) use this component.
 */

import { useNavigate, useParams } from "@tanstack/react-router"
import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useEffect, useState } from "react"
import { agentFamily, sessionNameFamily } from "../atoms/derived/agents"
import { markSessionReadAtom, upsertSessionAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { viewedSessionIdAtom } from "../atoms/ui"
import { useSessionRevert } from "../hooks/use-commands"
import type { ModelRef } from "../hooks/use-devo-data"
import { useConfig, useDevoAgents, useProviders, useVcs } from "../hooks/use-devo-data"
import { useAgentActions } from "../hooks/use-server"
import { useSessionChat } from "../hooks/use-session-chat"
import { createLogger } from "../lib/logger"
import type { Agent, FileAttachment, PermissionResponse, QuestionAnswer } from "../lib/types"
import { fetchSessionById } from "../services/connection-manager"
import { AgentDetail } from "./agent-detail"

const log = createLogger("session-view")

interface SessionViewProps {
	/** The Devo session ID to display */
	sessionId: string
}

export function SessionView({ sessionId }: SessionViewProps) {
	const navigate = useNavigate()
	const { projectSlug } = useParams({ strict: false }) as { projectSlug?: string }
	const {
		abort,
		sendPrompt,
		renameSession,
		respondToPermission,
		replyToQuestion,
		rejectQuestion,
		forkSession,
		deletePart,
	} = useAgentActions()

	// Track which session is currently viewed so background sessions can
	// skip expensive metric recomputation.
	const setViewedSessionId = useSetAtom(viewedSessionIdAtom)
	const markSessionRead = useSetAtom(markSessionReadAtom)
	useEffect(() => {
		setViewedSessionId(sessionId)
		markSessionRead(sessionId)
		return () => setViewedSessionId(null)
	}, [markSessionRead, sessionId, setViewedSessionId])

	const selectedAgent = useAtomValue(agentFamily(sessionId))

	// ── Fallback session fetch ──────────────────────────────────────────────
	// Subagent sessions are excluded from the initial batch load (roots:true)
	// and may also be missed if the Native event stream was reconnecting when the server
	// emitted session.created. If the session isn't in the Jotai store yet,
	// attempt a direct GET via the server's session.get endpoint so the user
	// isn't shown a dead "not found" screen.
	//
	// `resolving` stays true until either: (a) the agent is already in the
	// store (fast-path), (b) the fallback fetch succeeds and seeds the store,
	// or (c) the fetch fails / returns null (genuine not-found).
	const [resolving, setResolving] = useState(!selectedAgent)

	useEffect(() => {
		// Fast-path: session is already in the Jotai store.
		if (selectedAgent) {
			setResolving(false)
			return
		}

		// The session isn't in the store — attempt a server-side fetch.
		let cancelled = false
		setResolving(true)

		fetchSessionById(sessionId)
			.then((session) => {
				if (cancelled) return
				if (session) {
					// Seed into the Jotai store. agentFamily will derive an Agent from
					// this entry, causing selectedAgent to become non-null on the next
					// render, which in turn hits the fast-path above.
					appStore.set(upsertSessionAtom, {
						session,
						directory: session.directory ?? "",
					})
				} else {
					// Confirmed not found — stop resolving so "not found" renders.
					setResolving(false)
				}
			})
			.catch(() => {
				if (cancelled) return
				setResolving(false)
			})

		return () => {
			cancelled = true
		}
	}, [sessionId]) // Only re-run when the session ID changes (not on every agent update)

	// Resolve parent session name for breadcrumb navigation
	const parentSessionName = useAtomValue(sessionNameFamily(selectedAgent?.parentId ?? ""))

	// Load chat turns for the selected session
	const isSessionActive = selectedAgent?.status === "running" || selectedAgent?.status === "waiting"
	const {
		turns: chatTurns,
		loading: chatLoading,
		loadingEarlier: chatLoadingEarlier,
		hasEarlierMessages: chatHasEarlier,
		loadEarlier: chatLoadEarlier,
	} = useSessionChat(
		selectedAgent?.directory ?? null,
		selectedAgent?.sessionId ?? null,
		isSessionActive,
	)

	// Undo/redo for this session
	const { canUndo, canRedo, undo, redo, isReverted, revertToMessage } = useSessionRevert(
		selectedAgent?.directory ?? null,
		selectedAgent?.sessionId ?? null,
	)

	// Toolbar data -- providers, config, VCS, and Devo agents
	const directory = selectedAgent?.directory ?? null
	const { data: providers } = useProviders(directory)
	const { data: config } = useConfig(directory)
	const { data: vcs } = useVcs(directory)
	const { agents: devoAgents } = useDevoAgents(directory)

	// Handlers
	const handleStopAgent = useCallback(
		async (agent: Agent) => {
			await abort(agent.directory, agent.sessionId)
		},
		[abort],
	)

	const handleApprovePermission = useCallback(
		async (
			agent: Agent,
			permissionSessionId: string,
			permissionId: string,
			response?: PermissionResponse,
		) => {
			// Use permissionSessionId (not agent.sessionId) so that permissions from
			// sub-agent child sessions are correctly routed to the child's session.
			await respondToPermission(
				agent.directory,
				permissionSessionId,
				permissionId,
				response ?? "once",
			)
		},
		[respondToPermission],
	)

	const handleDenyPermission = useCallback(
		async (agent: Agent, permissionSessionId: string, permissionId: string) => {
			await respondToPermission(agent.directory, permissionSessionId, permissionId, "reject")
		},
		[respondToPermission],
	)

	const handleReplyQuestion = useCallback(
		async (agent: Agent, requestId: string, answers: QuestionAnswer[]) => {
			await replyToQuestion(agent.directory, requestId, answers)
		},
		[replyToQuestion],
	)

	const handleRejectQuestion = useCallback(
		async (agent: Agent, requestId: string) => {
			await rejectQuestion(agent.directory, requestId)
		},
		[rejectQuestion],
	)

	const handleRenameSession = useCallback(
		async (agent: Agent, title: string) => {
			await renameSession(agent.directory, agent.sessionId, title)
		},
		[renameSession],
	)

	const handleForkFromTurn = useCallback(
		async (messageId?: string) => {
			if (!selectedAgent) return
			try {
				const forked = await forkSession(selectedAgent.directory, selectedAgent.sessionId, messageId)
				if (forked && projectSlug) {
					navigate({
						to: "/project/$projectSlug/session/$sessionId",
						params: { projectSlug, sessionId: forked.id },
					})
				}
			} catch (err) {
				log.error("Fork failed", { sessionId: selectedAgent.sessionId, messageId }, err)
			}
		},
		[selectedAgent, forkSession, projectSlug, navigate],
	)

	const handleDeletePart = useCallback(
		async (sessionId: string, messageId: string, partId: string) => {
			if (!selectedAgent) return
			await deletePart(selectedAgent.directory, sessionId, messageId, partId)
		},
		[selectedAgent, deletePart],
	)

	const handleSendMessage = useCallback(
		async (
			agent: Agent,
			message: string,
			options?: {
				model?: ModelRef
				agentName?: string
				variant?: string
				files?: FileAttachment[]
				collaborationMode?: string
			},
		) => {
			log.debug("handleSendMessage", {
				sessionId: agent.sessionId,
				directory: agent.directory,
				messageLength: message.length,
				model: options?.model,
				agentName: options?.agentName,
				variant: options?.variant,
				collaborationMode: options?.collaborationMode,
			})
			try {
				await sendPrompt(agent.directory, agent.sessionId, message, {
					model: options?.model,
					agent: options?.agentName || undefined,
					variant: options?.variant,
					files: options?.files,
					collaborationMode: options?.collaborationMode,
				})
				log.debug("handleSendMessage completed", { sessionId: agent.sessionId })
			} catch (err) {
				log.error("handleSendMessage failed", { sessionId: agent.sessionId }, err)
				throw err
			}
		},
		[sendPrompt],
	)

	// Session not yet resolved — show spinner while the fallback fetch runs
	if (!selectedAgent && resolving) {
		return (
			<div className="flex h-full items-center justify-center">
				<div className="size-4 animate-spin rounded-full border-2 border-muted-foreground/20 border-t-muted-foreground/60" />
			</div>
		)
	}

	// Fallback fetch complete but session genuinely not found
	if (!selectedAgent) {
		return (
			<div className="flex h-full items-center justify-center">
				<div className="text-center">
					<p className="text-sm font-medium text-muted-foreground">Session not found</p>
					<p className="mt-1 text-xs text-muted-foreground/60">
						This session may have been deleted or is not yet loaded
					</p>
				</div>
			</div>
		)
	}

	return (
		<AgentDetail
			agent={selectedAgent}
			chatTurns={chatTurns}
			chatLoading={chatLoading}
			chatLoadingEarlier={chatLoadingEarlier}
			chatHasEarlier={chatHasEarlier}
			onLoadEarlier={chatLoadEarlier}
			onStop={handleStopAgent}
			onApprove={handleApprovePermission}
			onDeny={handleDenyPermission}
			onReplyQuestion={handleReplyQuestion}
			onRejectQuestion={handleRejectQuestion}
			onSendMessage={handleSendMessage}
			onRename={handleRenameSession}
			parentSessionName={parentSessionName}
			isConnected={true}
			providers={providers}
			config={config}
			vcs={vcs}
			devoAgents={devoAgents}
			canUndo={canUndo}
			canRedo={canRedo}
			onUndo={undo}
			onRedo={redo}
			isReverted={isReverted}
			onRevertToMessage={revertToMessage}
			onForkFromTurn={handleForkFromTurn}
			onDeletePart={handleDeletePart}
		/>
	)
}
