export interface SessionState {
	status: string
	title: string
	directory?: string
	parentID?: string
}

export function isWorkingSessionStatus(status: string | undefined): boolean {
	return status === "busy" || status === "retry"
}

export function shouldAnnounceCompletion(input: {
	hydrating: boolean
	isSubAgent: boolean
	previousStatus: string | undefined
	nextStatus: string
}): boolean {
	if (input.hydrating || input.isSubAgent) return false
	return input.nextStatus === "idle" && isWorkingSessionStatus(input.previousStatus)
}

export function shouldShowLiveNotification(hydrating: boolean): boolean {
	return !hydrating
}

export interface ForegroundWindowLike {
	isDestroyed(): boolean
	isVisible(): boolean
	isFocused(): boolean
	isMinimized(): boolean
}

/** True when a visible, focused, non-minimized window owns OS focus. */
export function isAppInForeground(windows: readonly ForegroundWindowLike[]): boolean {
	return windows.some(
		(win) => !win.isDestroyed() && win.isVisible() && !win.isMinimized() && win.isFocused(),
	)
}

export interface WatcherNotification {
	type: "permission" | "question" | "completed" | "error"
	sessionId: string
	title: string
	body: string
	directory?: string
	meta?: {
		permissionId?: string
		requestId?: string
	}
}

export interface GlobalNativeEvent {
	directory?: string
	payload?: {
		type: string
		properties: Record<string, unknown>
	}
}

export interface WatcherEventState {
	sessions: Map<string, SessionState>
	pendingCount: number
	hydrating: boolean
}

export interface WatcherEventResult {
	pendingCount: number
	notifications: WatcherNotification[]
	stateChanged: boolean
}

export function applyWatcherEvent(
	globalEvent: GlobalNativeEvent,
	state: WatcherEventState,
): WatcherEventResult {
	const event = globalEvent.payload
	if (!event) {
		return { pendingCount: state.pendingCount, notifications: [], stateChanged: false }
	}

	const props = event.properties
	const directory = globalEvent.directory
	const notifications: WatcherNotification[] = []
	let pendingCount = state.pendingCount
	let stateChanged = false

	switch (event.type) {
		case "permission.asked": {
			const sessionId = props.sessionID as string
			const permission = (props as { permission?: string }).permission
			const rootId = getRootSession(state.sessions, sessionId)
			const rootTitle = state.sessions.get(rootId)?.title
			const rootDir = state.sessions.get(rootId)?.directory ?? directory
			pendingCount++
			stateChanged = true
			if (shouldShowLiveNotification(state.hydrating)) {
				notifications.push({
					type: "permission",
					sessionId: rootId,
					title: isSubAgent(state.sessions, sessionId)
						? `Sub-agent needs permission${rootTitle ? ` — ${rootTitle}` : ""}`
						: "Agent needs permission",
					body: permission || "Approval required",
					directory: rootDir,
					meta: { permissionId: props.id as string },
				})
			}
			break
		}

		case "permission.replied": {
			pendingCount = Math.max(0, pendingCount - 1)
			stateChanged = true
			break
		}

		case "question.asked": {
			const sessionId = props.sessionID as string
			const questions = props.questions as Array<{ header?: string }> | undefined
			const header = questions?.[0]?.header ?? "Question"
			const rootId = getRootSession(state.sessions, sessionId)
			const rootTitle = state.sessions.get(rootId)?.title
			const rootDir = state.sessions.get(rootId)?.directory ?? directory
			pendingCount++
			stateChanged = true
			if (shouldShowLiveNotification(state.hydrating)) {
				notifications.push({
					type: "question",
					sessionId: rootId,
					title: isSubAgent(state.sessions, sessionId)
						? `Sub-agent has a question${rootTitle ? ` — ${rootTitle}` : ""}`
						: "Agent has a question",
					body: header,
					directory: rootDir,
					meta: { requestId: props.id as string },
				})
			}
			break
		}

		case "question.replied":
		case "question.rejected": {
			pendingCount = Math.max(0, pendingCount - 1)
			stateChanged = true
			break
		}

		case "session.status": {
			const sessionId = props.sessionID as string
			const newStatusType = (props.status as { type: string })?.type
			if (!sessionId || !newStatusType) break

			const prev = state.sessions.get(sessionId)
			const prevStatus = prev?.status
			state.sessions.set(sessionId, {
				status: newStatusType,
				title: prev?.title ?? "",
				directory: directory ?? prev?.directory,
				parentID: prev?.parentID,
			})
			stateChanged = true

			if (
				shouldAnnounceCompletion({
					hydrating: state.hydrating,
					isSubAgent: isSubAgent(state.sessions, sessionId),
					previousStatus: prevStatus,
					nextStatus: newStatusType,
				})
			) {
				const sessionTitle = state.sessions.get(sessionId)?.title
				notifications.push({
					type: "completed",
					sessionId,
					title: "Agent finished",
					body: sessionTitle || "Task completed",
					directory,
				})
			}
			break
		}

		case "session.error": {
			const sessionId = props.sessionID as string
			const error = props.error as { name?: string } | undefined
			if (!sessionId) break
			if (!isSubAgent(state.sessions, sessionId) && shouldShowLiveNotification(state.hydrating)) {
				notifications.push({
					type: "error",
					sessionId,
					title: "Agent encountered an error",
					body: error?.name ?? "Unknown error",
					directory,
				})
			}
			break
		}

		case "session.created":
		case "session.updated": {
			const info = (props.info ?? props.session) as
				| { id?: string; title?: string; parentID?: string }
				| undefined
			if (info?.id) {
				const existing = state.sessions.get(info.id)
				state.sessions.set(info.id, {
					status: existing?.status ?? "idle",
					title: info.title ?? existing?.title ?? "",
					directory: directory ?? existing?.directory,
					parentID: info.parentID ?? existing?.parentID,
				})
				stateChanged = true
			}
			break
		}

		default:
			break
	}

	return { pendingCount, notifications, stateChanged }
}

function isSubAgent(sessions: ReadonlyMap<string, SessionState>, sessionId: string): boolean {
	return !!sessions.get(sessionId)?.parentID
}

function getRootSession(sessions: ReadonlyMap<string, SessionState>, sessionId: string): string {
	let id = sessionId
	const seen = new Set<string>()
	while (true) {
		if (seen.has(id)) break
		seen.add(id)
		const parentID = sessions.get(id)?.parentID
		if (!parentID) break
		id = parentID
	}
	return id
}
