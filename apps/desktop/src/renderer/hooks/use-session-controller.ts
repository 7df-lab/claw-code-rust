import { useNavigate, useParams } from "@tanstack/react-router"
import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useEffect, useState } from "react"
import { agentFamily, sessionNameFamily } from "../atoms/derived/agents"
import { markSessionReadAtom, upsertSessionAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { viewedSessionIdAtom } from "../atoms/ui"
import { useSessionRevert } from "./use-commands"
import type { ModelRef } from "./use-devo-data"
import { useConfig, useDevoAgents, useProviders, useVcs } from "./use-devo-data"
import { useAgentActions } from "./use-server"
import { useSessionChat } from "./use-session-chat"
import { createLogger } from "../lib/logger"
import type { Agent, FileAttachment, PermissionResponse, QuestionAnswer } from "../lib/types"
import { fetchSessionById } from "../services/connection-manager"

const log = createLogger("session-controller")

export function useSessionController(sessionId: string, isActive = true) {
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
		editMessage,
	} = useAgentActions()

	const setViewedSessionId = useSetAtom(viewedSessionIdAtom)
	const markSessionRead = useSetAtom(markSessionReadAtom)
	useEffect(() => {
		if (!isActive) return
		setViewedSessionId(sessionId)
		markSessionRead(sessionId)
		return () => setViewedSessionId(null)
	}, [isActive, markSessionRead, sessionId, setViewedSessionId])

	const agent = useAtomValue(agentFamily(sessionId))
	const [resolving, setResolving] = useState(!agent)

	useEffect(() => {
		if (agent) {
			setResolving(false)
			return
		}

		let cancelled = false
		setResolving(true)

		fetchSessionById(sessionId)
			.then((session) => {
				if (cancelled) return
				if (session) {
					appStore.set(upsertSessionAtom, {
						session,
						directory: session.directory ?? "",
					})
				} else {
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
	}, [sessionId])

	const parentSessionName = useAtomValue(
		sessionNameFamily(agent?.parentId ?? agent?.forkFromId ?? ""),
	)

	const {
		turns: chatTurns,
		loading: chatLoading,
		showLoading: chatShowLoading,
		loadingEarlier: chatLoadingEarlier,
		hasEarlierMessages: chatHasEarlier,
		loadEarlier: chatLoadEarlier,
	} = useSessionChat(agent?.directory ?? null, agent?.sessionId ?? null, isActive)

	const { canUndo, canRedo, undo, redo, isReverted } = useSessionRevert(
		agent?.directory ?? null,
		agent?.sessionId ?? null,
	)

	const directory = agent?.directory ?? null
	const { data: providers } = useProviders(directory)
	const { data: config } = useConfig(directory)
	const { data: vcs } = useVcs(directory)
	const { agents: devoAgents } = useDevoAgents(directory)

	const handleStopAgent = useCallback(
		async (target: Agent) => {
			await abort(target.directory, target.sessionId)
		},
		[abort],
	)

	const handleApprovePermission = useCallback(
		async (
			target: Agent,
			permissionSessionId: string,
			permissionId: string,
			response?: PermissionResponse,
		) => {
			await respondToPermission(
				target.directory,
				permissionSessionId,
				permissionId,
				response ?? "once",
			)
		},
		[respondToPermission],
	)

	const handleDenyPermission = useCallback(
		async (target: Agent, permissionSessionId: string, permissionId: string) => {
			await respondToPermission(target.directory, permissionSessionId, permissionId, "reject")
		},
		[respondToPermission],
	)

	const handleReplyQuestion = useCallback(
		async (target: Agent, requestId: string, answers: QuestionAnswer[]) => {
			await replyToQuestion(target.directory, requestId, answers)
		},
		[replyToQuestion],
	)

	const handleRejectQuestion = useCallback(
		async (target: Agent, requestId: string) => {
			await rejectQuestion(target.directory, requestId)
		},
		[rejectQuestion],
	)

	const handleRenameSession = useCallback(
		async (target: Agent, title: string) => {
			await renameSession(target.directory, target.sessionId, title)
		},
		[renameSession],
	)

	const handleForkFromTurn = useCallback(
		async (turnId?: string) => {
			if (!agent) return
			try {
				const forked = await forkSession(agent.directory, agent.sessionId, {
					atTurnId: turnId,
				})
				if (forked && projectSlug) {
					navigate({
						to: "/project/$projectSlug/session/$sessionId",
						params: { projectSlug, sessionId: forked.id },
					})
				}
			} catch (err) {
				log.error("Fork failed", { sessionId: agent.sessionId, turnId }, err)
			}
		},
		[agent, forkSession, projectSlug, navigate],
	)

	const handleDeletePart = useCallback(
		async (targetSessionId: string, messageId: string, partId: string) => {
			if (!agent) return
			await deletePart(agent.directory, targetSessionId, messageId, partId)
		},
		[agent, deletePart],
	)

	const handleEditUserMessage = useCallback(
		async (messageId: string, text: string) => {
			if (!agent) return
			await editMessage(agent.directory, agent.sessionId, messageId, text)
		},
		[agent, editMessage],
	)

	const handleSendMessage = useCallback(
		async (
			target: Agent,
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
				sessionId: target.sessionId,
				directory: target.directory,
				messageLength: message.length,
				model: options?.model,
				agentName: options?.agentName,
				variant: options?.variant,
				collaborationMode: options?.collaborationMode,
			})
			try {
				await sendPrompt(target.directory, target.sessionId, message, {
					model: options?.model,
					agent: options?.agentName || undefined,
					variant: options?.variant,
					files: options?.files,
					collaborationMode: options?.collaborationMode,
				})
				log.debug("handleSendMessage completed", { sessionId: target.sessionId })
			} catch (err) {
				log.error("handleSendMessage failed", { sessionId: target.sessionId }, err)
				throw err
			}
		},
		[sendPrompt],
	)

	return {
		agent,
		resolving,
		parentSessionName,
		chatTurns,
		chatLoading,
		chatShowLoading,
		chatLoadingEarlier,
		chatHasEarlier,
		chatLoadEarlier,
		providers,
		config,
		vcs,
		devoAgents,
		canUndo,
		canRedo,
		undo,
		redo,
		isReverted,
		handleStopAgent,
		handleApprovePermission,
		handleDenyPermission,
		handleReplyQuestion,
		handleRejectQuestion,
		handleRenameSession,
		handleForkFromTurn,
		handleDeletePart,
		handleEditUserMessage,
		handleSendMessage,
	}
}
