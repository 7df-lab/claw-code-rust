import { MessageResponse } from "@devo/ui/components/ai-elements/message"
import { cn } from "@devo/ui/lib/utils"
import { useNavigate, useParams } from "@tanstack/react-router"
import { useAtomValue } from "jotai"
import {
	ArrowRightIcon,
	ChevronDownIcon,
	ChevronRightIcon,
	ChevronUpIcon,
	Loader2Icon,
	MessageCircleQuestionIcon,
	ShieldAlertIcon,
	ZapIcon,
} from "lucide-react"
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { messagesFamily } from "../../atoms/messages"
import { partsFamily, partStorageKey } from "../../atoms/parts"
import { sessionFamily } from "../../atoms/sessions"
import { appStore } from "../../atoms/store"
import { getStreamingPartsForSession, streamingVersionFamily } from "../../atoms/streaming"
import { useToolElapsedTime } from "../../hooks/use-elapsed-time"
import type { ToolPart, ToolState } from "../../lib/types"
import { getToolInfo, getToolSubtitle } from "./chat-tool-call"

// ============================================================
// Collapse state for three-tier agent card
// ============================================================

type CollapseState = "closed" | "summary" | "expanded"

/**
 * Extract the first meaningful plain-text line from a markdown string.
 * Strips headings, horizontal rules, bold/italic markers, link syntax,
 * and skips blank/decoration-only lines.
 */
function extractFirstLine(md: string): string | undefined {
	const lines = md.split("\n")
	for (const raw of lines) {
		const line = raw
			.replace(/^#{1,6}\s+/, "") // strip heading markers
			.replace(/^[-*_]{3,}\s*$/, "") // strip horizontal rules
			.replace(/\*{1,2}([^*]+)\*{1,2}/g, "$1") // strip bold/italic
			.replace(/`([^`]+)`/g, "$1") // strip inline code
			.replace(/\[([^\]]+)\]\([^)]+\)/g, "$1") // strip links → keep label
			.replace(/^[-*+]\s+/, "") // strip list markers
			.replace(/^\d+\.\s+/, "") // strip ordered list markers
			.trim()
		if (line.length > 0) return line
	}
	return undefined
}

// ============================================================
// SubAgentCard
// ============================================================

interface SubAgentCardProps {
	part: ToolPart
	/** Project root for display-only relative paths on nested tool rows. */
	projectRoot?: string | null
}

/**
 * Renders a sub-agent (task tool) as a three-state collapsible card.
 *
 * **Closed**: Header bar only — chevron, Zap icon, "Agent" label,
 * agent type, truncated task description, live status / duration, Open button.
 *
 * **Summary**: Header + first ~4 lines of the agent's final text as a
 * preview with a "Show more" affordance. No tool rows shown.
 *
 * **Expanded**: Header + task description + tool activity rows + full
 * markdown-rendered agent response.
 *
 * While running the card is fully expanded. On completion it auto-collapses
 * to the summary state (or closed if there's no text).
 */
export const SubAgentCard = memo(function SubAgentCard({
	part: propPart,
	projectRoot,
}: SubAgentCardProps) {
	const navigate = useNavigate()
	const { projectSlug } = useParams({ strict: false }) as { projectSlug?: string }

	// Read the live tool part directly from the store so we always have the
	// latest state (status, metadata, output) even when the parent turn's
	// structural sharing keeps the prop stale.
	const messageParts = useAtomValue(
		partsFamily(partStorageKey(propPart.sessionID, propPart.messageID)),
	)
	const livePart = useMemo(
		() => messageParts?.find((p): p is ToolPart => p.id === propPart.id && p.type === "tool"),
		[messageParts, propPart.id],
	)
	const part = livePart ?? propPart

	// Count how many sibling task tools exist in the same message.
	// When there are parallel sub-agents, we start in "summary" instead of
	// "expanded" to avoid a noisy wall of live tool activity.
	const hasParallelSiblings = useMemo(
		() => (messageParts?.filter((p) => p.type === "tool" && p.tool === "task").length ?? 0) > 1,
		[messageParts],
	)

	// Derive sessionId from the live part's metadata so it becomes available
	// as soon as the server populates it, even if the parent doesn't re-render.
	const sessionId = useMemo(() => {
		if (part.tool !== "task") return undefined
		const state = part.state as ToolState & { metadata?: Record<string, unknown> }
		return (state.metadata?.sessionId as string | undefined) ?? undefined
	}, [part])

	const handleNavigate = useCallback(
		(e: React.MouseEvent) => {
			// Prevent the click from toggling the collapsible
			e.stopPropagation()
			if (sessionId) {
				navigate({
					to: "/project/$projectSlug/session/$sessionId",
					params: {
						projectSlug: projectSlug ?? "unknown",
						sessionId,
					},
				})
			}
		},
		[sessionId, navigate, projectSlug],
	)

	const taskTitle =
		(part.state.input?.description as string) ??
		("title" in part.state ? part.state.title : undefined) ??
		"Sub-agent"
	const agentType = (part.state.input?.subagent_type as string) ?? "general"

	// Determine if the sub-agent is still running
	const isRunning = part.state.status === "running" || part.state.status === "pending"
	const isError = part.state.status === "error"
	const isCompleted = part.state.status === "completed"

	// Detect pending interactive requests (permissions / questions) on the child session.
	// This lets the card header show a "waiting" indicator while the user hasn't
	// yet responded in the parent session's input area.
	const childSessionEntry = useAtomValue(sessionFamily(sessionId ?? ""))
	const childHasPendingPermission = (childSessionEntry?.permissions.length ?? 0) > 0
	const childHasPendingQuestion = (childSessionEntry?.questions.length ?? 0) > 0
	const childIsWaiting = isRunning && (childHasPendingPermission || childHasPendingQuestion)

	// ── Three-state collapse ───────────────────────────────────
	// "closed"   → header only
	// "summary"  → header + text preview (first ~4 lines)
	// "expanded" → header + tools + full markdown text
	const [collapseState, setCollapseState] = useState<CollapseState>(
		hasParallelSiblings ? "summary" : "expanded",
	)
	const wasRunningRef = useRef(isRunning)

	useEffect(() => {
		// When transitioning from running → completed/error,
		// auto-collapse to "summary" if there's text, otherwise "closed"
		if (wasRunningRef.current && !isRunning) {
			// We defer to next render so latestText is populated
			requestAnimationFrame(() => {
				setCollapseState((prev) => (prev === "expanded" ? "summary" : prev))
			})
		}
		wasRunningRef.current = isRunning
	}, [isRunning])

	const handleHeaderToggle = useCallback(() => {
		setCollapseState((prev) => {
			if (prev === "closed") return isRunning ? "expanded" : "summary"
			// Both "summary" and "expanded" collapse to "closed"
			return "closed"
		})
	}, [isRunning])

	const handleShowMore = useCallback((e: React.MouseEvent) => {
		e.stopPropagation()
		setCollapseState("expanded")
	}, [])

	const handleShowLess = useCallback((e: React.MouseEvent) => {
		e.stopPropagation()
		setCollapseState("summary")
	}, [])

	// ── Elapsed time (live, while running) ─────────────────────
	const elapsedTime = useToolElapsedTime(part)

	// Access child session data from the store.
	const childMessages = useAtomValue(messagesFamily(sessionId ?? ""))

	// Subscribe to the per-session streaming version so we only re-render
	// when this child session streams, not when any other session streams.
	const streamingVersion = useAtomValue(streamingVersionFamily(sessionId ?? ""))

	// Derive child session's activity.
	// Optimized: iterates messages backwards and collects only what's needed
	// (last 3 tool parts, last text, status) without building a full allParts array.
	const { latestToolParts, latestText, childStatus } = useMemo(() => {
		if (!childMessages || childMessages.length === 0) {
			return { latestToolParts: [], latestText: undefined, childStatus: undefined }
		}

		// Reference streamingVersion so the linter sees it as used and it triggers recomputation.
		void streamingVersion
		const streaming = getStreamingPartsForSession(sessionId ?? "")

		// Single-pass collection: walk all messages forward, but only keep
		// the last 3 tool parts and overwrite latestText + status as we go.
		// This avoids building a temporary allParts array.
		const toolParts: ToolPart[] = []
		let latestText: string | undefined
		let lastStatus: string | undefined

		for (const msg of childMessages) {
			const baseParts = appStore.get(partsFamily(partStorageKey(sessionId ?? "", msg.id)))
			if (!baseParts) continue
			const overrides = streaming[msg.id]

			for (const bp of baseParts) {
				const p = overrides?.[bp.id] ?? bp
				if (p.type === "tool" && p.tool !== "todoread") {
					toolParts.push(p)
				}
				// Track last text part (overwrites as we move forward)
				if (p.type === "text" && !p.synthetic && p.text.trim()) {
					latestText = p.text.trim()
				}
				// Track status from the last meaningful part
				if (p.type === "tool") {
					switch (p.tool) {
						case "task":
							lastStatus = "Delegating..."
							break
						case "todowrite":
						case "todoread":
							lastStatus = "Planning..."
							break
						case "read":
							lastStatus = "Reading files..."
							break
						case "list":
						case "grep":
						case "glob":
							lastStatus = "Searching codebase..."
							break
						case "webfetch":
							lastStatus = "Fetching web content..."
							break
						case "edit":
						case "write":
						case "apply_patch":
							lastStatus = "Making edits..."
							break
						case "bash":
						case "shell_command":
						case "exec_command":
							lastStatus = ""
							break
						default:
							lastStatus = `Running ${p.tool}...`
							break
					}
				} else if (p.type === "reasoning") {
					lastStatus = "Thinking..."
				} else if (p.type === "text") {
					lastStatus = "Composing response..."
				}
			}
		}

		return {
			latestToolParts: toolParts.slice(-3),
			latestText,
			childStatus: lastStatus ?? "Working...",
		}
	}, [childMessages, streamingVersion, sessionId])

	// Extract first meaningful line for the summary teaser.
	// "hasMore" is true when the full text has content beyond the first line.
	const firstLine = useMemo(() => extractFirstLine(latestText ?? ""), [latestText])
	const hasMore = useMemo(() => {
		if (!latestText || !firstLine) return false
		// If there's any non-trivial content beyond the first line, we have more
		const rest = latestText.slice(latestText.indexOf(firstLine) + firstLine.length).trim()
		return rest.length > 0
	}, [latestText, firstLine])

	// If there's no text and we're in summary mode, fall back to closed
	// (summary with nothing to show is pointless)
	useEffect(() => {
		if (collapseState === "summary" && !firstLine) {
			setCollapseState("closed")
		}
	}, [collapseState, firstLine])

	const showSummary = collapseState === "summary" || collapseState === "expanded"
	const showExpanded = collapseState === "expanded"

	const ChevronIcon = collapseState !== "closed" ? ChevronDownIcon : ChevronRightIcon

	return (
		<div className="not-prose space-y-0">
			<div className="flex w-full max-w-full items-center gap-1.5">
				<button
					type="button"
					onClick={handleHeaderToggle}
					className="-mx-1.5 flex min-w-0 flex-1 items-center gap-1.5 rounded px-1.5 py-0.5 text-left text-sm text-muted-foreground/70 transition-colors hover:bg-muted/40 hover:text-foreground"
				>
					<ChevronIcon aria-hidden="true" className="size-3.5 shrink-0 transition-transform" />
					<ZapIcon
						className={cn(
							"size-3.5 shrink-0 stroke-[1.5]",
							isRunning ? "animate-pulse text-violet-400" : "text-muted-foreground/50",
						)}
					/>
					<span className="min-w-0 truncate">
						<span className={isError ? "text-red-400" : undefined}>Agent</span>
						<span className="text-muted-foreground/60"> ({agentType})</span>
						<span className="text-muted-foreground/60"> · {taskTitle}</span>
					</span>
				</button>
				<div className="ml-auto flex shrink-0 items-center gap-2">
				{/* Waiting indicator: shown when sub-agent has a pending permission or question */}
				{childIsWaiting && childHasPendingPermission && (
					<span className="flex items-center gap-1 text-[11px] font-medium text-amber-400">
						<ShieldAlertIcon className="size-3 shrink-0" aria-hidden="true" />
						Needs approval
					</span>
				)}
				{childIsWaiting && childHasPendingQuestion && !childHasPendingPermission && (
					<span className="flex items-center gap-1 text-[11px] font-medium text-amber-400">
						<MessageCircleQuestionIcon className="size-3 shrink-0" aria-hidden="true" />
						Asking a question
					</span>
				)}
				{isRunning && !childIsWaiting && childStatus && (
					<span className="text-[11px] text-muted-foreground/60">{childStatus}</span>
				)}
				{isRunning && elapsedTime && (
					<span className="text-[11px] tabular-nums text-muted-foreground/40">
						{elapsedTime}
					</span>
				)}
				{isRunning && !childIsWaiting && <Loader2Icon className="size-3 animate-spin text-muted-foreground/40" />}
				{childIsWaiting && <Loader2Icon className="size-3 animate-spin text-amber-400/60" />}
					{sessionId && (
						<button
							type="button"
							onClick={handleNavigate}
							className="inline-flex items-center gap-0.5 rounded px-1.5 py-0.5 text-[11px] font-medium text-primary transition-colors hover:bg-primary/10"
						>
							Open
							<ArrowRightIcon className="size-3" />
						</button>
					)}
				</div>
			</div>

			{showSummary && (
				<div className="ml-[7px] mt-1.5 mb-2 border-l border-border/40 pl-3">
					{!showExpanded && firstLine && (
						<div className="flex items-baseline gap-2 py-2 pr-1">
							<p className="min-w-0 flex-1 truncate text-[11px] leading-relaxed text-muted-foreground/70 italic">
								{firstLine}
							</p>
							{hasMore && (
								<button
									type="button"
									onClick={handleShowMore}
									className="inline-flex shrink-0 items-center gap-0.5 text-[10px] font-medium text-primary/70 transition-colors hover:text-primary"
								>
									Show more
									<ChevronDownIcon className="size-3" />
								</button>
							)}
						</div>
					)}

					{showExpanded && (
						<>
					{/* Live activity: latest tool calls */}
					{latestToolParts.length > 0 && (
						<div className="py-2 pr-1">
							<div className="space-y-1">
								{latestToolParts.map((tp) => {
									const tpRunning = tp.state.status === "running" || tp.state.status === "pending"
									const { icon: TpIcon, title } = getToolInfo(tp.tool, {
										running: tpRunning,
										input: tp.state.input as Record<string, unknown> | undefined,
									})
									const tpSubtitle = getToolSubtitle(tp, { projectRoot })
									const tpError = tp.state.status === "error"

									return (
										<div
											key={tp.id}
											className={cn(
												"flex items-center gap-2 py-1 text-[11px]",
											)}
										>
											<TpIcon
												className={cn(
													"size-3.5 shrink-0 stroke-[1.5]",
													tpError
														? "text-red-400"
														: tpRunning
															? "text-muted-foreground animate-pulse"
															: "text-muted-foreground/60",
												)}
											/>
											<span
												className={cn(
													"shrink-0 font-medium",
													tpError ? "text-red-400" : "text-foreground/70",
												)}
											>
												{title}
											</span>
											{tpSubtitle && (
												<span className="min-w-0 truncate font-mono text-muted-foreground/50">
													{tpSubtitle}
												</span>
											)}
										</div>
									)
								})}
							</div>
						</div>
					)}

					{/* Full agent response rendered as markdown */}
					{latestText && (
						<div className="py-2 pr-1">
							<div className="max-h-96 overflow-y-auto text-xs text-muted-foreground">
								<MessageResponse
									animated={isRunning}
									className="[&>*:first-child]:mt-0 [&>*:last-child]:mb-0 [&_h1]:text-sm [&_h2]:text-xs [&_h3]:text-xs [&_li]:text-xs [&_p]:text-xs [&_p]:my-1 [&_pre]:max-h-40 [&_pre]:text-[11px]"
								>
									{latestText}
								</MessageResponse>
							</div>
							{hasMore && (
								<button
									type="button"
									onClick={handleShowLess}
									className="mt-2 inline-flex items-center gap-0.5 text-[10px] font-medium text-primary/70 transition-colors hover:text-primary"
								>
									Show less
									<ChevronUpIcon className="size-3" />
								</button>
							)}
						</div>
					)}

					{/* Completion / error state */}
					{isCompleted && !latestToolParts.length && !latestText && (
						<div className="py-2 pr-1">
							<span className="text-[11px] text-muted-foreground/50">Completed</span>
						</div>
					)}
					{isError && (
						<div className="mt-1 rounded bg-red-500/5 px-2.5 py-2">
							<span className="text-[11px] text-red-400">
								{part.state.status === "error" ? part.state.error : "Sub-agent failed"}
							</span>
						</div>
					)}
						</>
					)}

					{!showExpanded && isError && (
						<div className="mt-1 rounded bg-red-500/5 px-2.5 py-2">
							<span className="text-[11px] text-red-400">
								{part.state.status === "error" ? part.state.error : "Sub-agent failed"}
							</span>
						</div>
					)}
				</div>
			)}
		</div>
	)
})
