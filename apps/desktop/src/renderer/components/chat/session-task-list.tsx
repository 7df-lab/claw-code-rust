import { cn } from "@devo/ui/lib/utils"
import { useAtomValue } from "jotai"
import {
	CheckCircle2Icon,
	ChevronDownIcon,
	ChevronUpIcon,
	CircleDotIcon,
	Loader2Icon,
	XCircleIcon,
} from "lucide-react"
import { useEffect, useMemo, useRef, useState } from "react"
import { messagesFamily } from "../../atoms/messages"
import { partStorageKey, partsFamily } from "../../atoms/parts"
import { appStore } from "../../atoms/store"
import { streamingVersionFamily } from "../../atoms/streaming"
import { todosFamily } from "../../atoms/todos"
import type { Todo } from "../../lib/types"

function normalizeTodoStatus(status: string): string {
	switch (status) {
		case "completed":
			return "completed"
		case "in_progress":
		case "inProgress":
			return "in_progress"
		case "cancelled":
			return "cancelled"
		default:
			return "pending"
	}
}

function todosFromPlanPart(part: { type: string; metadata?: Record<string, unknown> }): Todo[] | null {
	if (part.type !== "text") return null
	const kind = part.metadata?.["devo/itemKind"]
	if (kind !== "plan") return null
	const raw = part.metadata?.planEntries
	if (!Array.isArray(raw) || raw.length === 0) return null
	const todos = raw
		.map((entry) => {
			if (!entry || typeof entry !== "object") return null
			const value = entry as Record<string, unknown>
			const content = String(value.content ?? value.step ?? "").trim()
			if (!content) return null
			// Skip expanded-looking markdown proposed-plan blobs.
			if (content.includes("\n") && !content.trim().startsWith("{")) return null
			return {
				content,
				status: normalizeTodoStatus(String(value.status ?? "pending")),
			} as Todo
		})
		.filter((todo): todo is Todo => todo !== null)
	return todos.length > 0 ? todos : null
}

/**
 * Derives the latest todo list for a session.
 *
 * Priority order:
 * 1. Store `todos[sessionId]` — set by `todo.updated` Native events (real-time)
 * 2. Fallback: last Native `plan` text part's `planEntries` (session reload)
 * 3. Fallback: last `todowrite` tool part (legacy)
 */
function useSessionTodos(sessionId: string | null): Todo[] {
	const storeTodos = useAtomValue(todosFamily(sessionId ?? ""))
	const storeMessages = useAtomValue(messagesFamily(sessionId ?? ""))
	const streamingVersion = useAtomValue(streamingVersionFamily(sessionId ?? ""))

	return useMemo(() => {
		// If we have Native-pushed todos, prefer those — they're the most up-to-date
		if (storeTodos && storeTodos.length > 0) return storeTodos

		// Fallback: walk messages backwards for plan entries or legacy todowrite
		if (!storeMessages || storeMessages.length === 0) return []
		// streamingVersion in deps triggers recomputation when parts update
		void streamingVersion
		const sid = sessionId ?? ""
		for (let i = storeMessages.length - 1; i >= 0; i--) {
			const msg = storeMessages[i]
			const parts = appStore.get(partsFamily(partStorageKey(sid, msg.id)))
			if (!parts) continue
			for (let j = parts.length - 1; j >= 0; j--) {
				const part = parts[j]
				const fromPlan = todosFromPlanPart(part)
				if (fromPlan) return fromPlan
				if (part.type === "tool" && part.tool === "todowrite") {
					const todos = part.state.input?.todos as Todo[] | undefined
					if (todos && todos.length > 0) {
						return todos.map((todo) => ({
							...todo,
							status: normalizeTodoStatus(String(todo.status ?? "pending")),
						}))
					}
				}
			}
		}
		return []
	}, [storeTodos, storeMessages, streamingVersion, sessionId])
}

/** Compact status icon for a todo item */
function TodoStatusIcon({ status }: { status: string }) {
	switch (normalizeTodoStatus(status)) {
		case "completed":
			return <CheckCircle2Icon className="size-3.5 text-emerald-500/80" />
		case "in_progress":
			return <Loader2Icon className="size-3.5 animate-spin text-blue-400/80" />
		case "cancelled":
			return <XCircleIcon className="size-3.5 text-muted-foreground/30" />
		default:
			return <CircleDotIcon className="size-3.5 text-muted-foreground/30" />
	}
}

interface SessionTaskListProps {
	sessionId: string | null
}

/**
 * Collapsible task list that appears above the input field.
 * Shows the session's current todo list.
 * Subtly styled; task items animate in with stagger and re-animate on status change.
 */
export function SessionTaskList({ sessionId }: SessionTaskListProps) {
	const todos = useSessionTodos(sessionId)
	const [isExpanded, setIsExpanded] = useState(true)
	const scrollRef = useRef<HTMLDivElement>(null)

	const activeTask = useMemo(
		() => todos.find((t) => normalizeTodoStatus(t.status) === "in_progress"),
		[todos],
	)

	const headerLabel = activeTask?.content ?? "Tasks"

	// Auto-scroll to bottom when todos change
	// biome-ignore lint/correctness/useExhaustiveDependencies: scroll on todo changes intentionally
	useEffect(() => {
		if (isExpanded && scrollRef.current) {
			scrollRef.current.scrollTo({ top: scrollRef.current.scrollHeight, behavior: "smooth" })
		}
	}, [todos, isExpanded])

	if (todos.length === 0) return null

	return (
		<div className="mb-2 animate-in fade-in overflow-hidden rounded-lg border border-border/60 bg-card shadow-sm duration-400">
			{/* Header — always visible, toggles expansion */}
			<button
				type="button"
				onClick={() => setIsExpanded((prev) => !prev)}
				aria-expanded={isExpanded}
				className={cn(
					"flex w-full items-center gap-2.5 bg-card px-3 py-1.5 text-left transition-colors hover:bg-muted/40",
					isExpanded ? "rounded-t-lg" : "rounded-lg",
				)}
			>
				<span className="min-w-0 flex-1 truncate text-sm text-foreground/80">
					{isExpanded ? "Tasks" : headerLabel}
				</span>

				{/* Chevron indicator */}
				{isExpanded ? (
					<ChevronDownIcon
						className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground/60"
						aria-hidden="true"
					/>
				) : (
					<ChevronUpIcon
						className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground/60"
						aria-hidden="true"
					/>
				)}
			</button>

			{/* Expandable task list — smooth height transition via grid trick */}
			<div
				className={cn(
					"grid transition-[grid-template-rows] duration-200 ease-out",
					isExpanded ? "grid-rows-[1fr]" : "grid-rows-[0fr]",
				)}
			>
				<div className="overflow-hidden">
					<div
						ref={scrollRef}
						className="max-h-44 overflow-y-auto border-t border-border/30 px-3 pb-2 pt-1.5"
					>
						<ol className="space-y-1">
							{todos.map((todo, index) => (
								// Key includes status so item re-mounts (fades in fresh) on status change
								// biome-ignore lint/suspicious/noArrayIndexKey: no stable ID in SDK todos
								<li
									key={`${index}-${todo.status}`}
									className="flex items-start gap-2 animate-in fade-in-0 slide-in-from-bottom-1 duration-300"
									style={{ animationDelay: `${index * 35}ms`, animationFillMode: "backwards" }}
								>
									<span className="mt-0.5 shrink-0">
										<TodoStatusIcon status={todo.status} />
									</span>
									<span className="flex items-baseline gap-1.5 text-sm leading-relaxed">
										<span className="shrink-0 tabular-nums text-muted-foreground/40">{index + 1}.</span>
										<span
											className={cn(
												"transition-colors duration-300",
												todo.status === "completed"
													? "text-muted-foreground/50 line-through"
													: todo.status === "cancelled"
														? "text-muted-foreground/40 line-through"
														: todo.status === "in_progress"
															? "text-foreground"
															: "text-muted-foreground",
											)}
										>
											{todo.content}
										</span>
									</span>
								</li>
							))}
						</ol>
					</div>
				</div>
			</div>
		</div>
	)
}
