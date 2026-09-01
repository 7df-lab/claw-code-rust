/**
 * Session-header context occupancy control.
 *
 * The trigger lives in the session header. The panel portals into the
 * conversation surface so it matches the transcript column width and sits
 * flush with the top of the chat area, using the same popover chrome as
 * the rest of the desktop UI.
 */
import { cn } from "@devo/ui/lib/utils"
import { useAtom, useAtomValue, useSetAtom } from "jotai"
import { XIcon } from "lucide-react"
import { useEffect, useLayoutEffect, useMemo, useState } from "react"
import { createPortal } from "react-dom"
import { sessionNativeFamily } from "../atoms/session-native"
import { contextUsageOpenAtom, reviewPanelOpenAtom } from "../atoms/ui"
import {
	occupancyCategoryRows,
	windowFillPercent,
	type ContextCategoryId,
} from "../lib/context-occupancy"
import { formatTokens } from "../lib/session-metrics"

const CATEGORY_COLORS: Record<ContextCategoryId, string> = {
	base: "bg-muted-foreground/45",
	skills: "bg-chart-1",
	toolsBuiltin: "bg-chart-2",
	toolsMcp: "bg-chart-4",
	conversation: "bg-chart-3",
}

/** Matches ConversationContent horizontal gutters; flush to the top of the surface. */
export const CONTEXT_USAGE_GUTTER_CLASS = "px-6 sm:px-10 lg:px-12"

interface ContextUsageButtonProps {
	sessionId: string
}

export function ContextUsageButton({ sessionId }: ContextUsageButtonProps) {
	const [open, setOpen] = useAtom(contextUsageOpenAtom)
	const native = useAtomValue(sessionNativeFamily(sessionId))
	const occupancy = native.occupancy
	const used = occupancy?.totalTokens ?? Number(native.usage?.used ?? 0)
	const windowTokens = occupancy?.contextWindowTokens ?? Number(native.usage?.size ?? 0)
	const percent = windowFillPercent(used, windowTokens)
	const rows = useMemo(() => occupancyCategoryRows(occupancy), [occupancy])

	useEffect(() => {
		setOpen(false)
	}, [sessionId, setOpen])

	const size = 14
	const strokeWidth = 2.5
	const radius = (size - strokeWidth) / 2
	const circumference = 2 * Math.PI * radius
	const offset = circumference - (Math.min(percent, 100) / 100) * circumference

	return (
		<>
			<button
				type="button"
				data-context-usage-trigger=""
				aria-expanded={open}
				aria-label={`Context usage ${percent}%`}
				onClick={() => setOpen((current) => !current)}
				className={cn(
					"inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground",
					open && "bg-muted text-foreground",
				)}
			>
				<svg
					width={size}
					height={size}
					viewBox={`0 0 ${size} ${size}`}
					className="shrink-0"
					aria-hidden="true"
				>
					<circle
						cx={size / 2}
						cy={size / 2}
						r={radius}
						fill="none"
						className="stroke-current opacity-25"
						strokeWidth={strokeWidth}
					/>
					<circle
						cx={size / 2}
						cy={size / 2}
						r={radius}
						fill="none"
						className="stroke-current"
						strokeWidth={strokeWidth}
						strokeDasharray={circumference}
						strokeDashoffset={offset}
						strokeLinecap="round"
						transform={`rotate(-90 ${size / 2} ${size / 2})`}
					/>
				</svg>
			</button>
			{open ? (
				<ContextUsageOverlay
					percent={percent}
					rows={rows}
					sessionId={sessionId}
					used={used}
					windowTokens={windowTokens}
				/>
			) : null}
		</>
	)
}

function ContextUsageOverlay({
	sessionId,
	used,
	windowTokens,
	percent,
	rows,
}: {
	sessionId: string
	used: number
	windowTokens: number
	percent: number
	rows: ReturnType<typeof occupancyCategoryRows>
}) {
	const setOpen = useSetAtom(contextUsageOpenAtom)
	const reviewPanelOpen = useAtomValue(reviewPanelOpenAtom)
	const [container, setContainer] = useState<HTMLElement | null>(null)
	const filledRows = rows.filter((row) => row.tokens > 0)
	const widthClass = reviewPanelOpen
		? "mx-auto w-full min-w-0"
		: "mx-auto w-full min-w-0 max-w-3xl"

	useLayoutEffect(() => {
		const find = () =>
			document.querySelector(
				`[data-conversation-surface="${CSS.escape(sessionId)}"]`,
			) as HTMLElement | null
		setContainer(find())
		const frame = requestAnimationFrame(() => setContainer(find()))
		return () => cancelAnimationFrame(frame)
	}, [sessionId])

	useEffect(() => {
		const close = () => setOpen(false)
		const onKeyDown = (event: KeyboardEvent) => {
			if (event.key === "Escape") {
				event.preventDefault()
				close()
			}
		}
		const onPointerDown = (event: PointerEvent) => {
			const target = event.target
			if (!(target instanceof Element)) return
			if (target.closest("[data-context-usage-panel]")) return
			if (target.closest("[data-context-usage-trigger]")) return
			close()
		}
		document.addEventListener("keydown", onKeyDown)
		document.addEventListener("pointerdown", onPointerDown)
		return () => {
			document.removeEventListener("keydown", onKeyDown)
			document.removeEventListener("pointerdown", onPointerDown)
		}
	}, [setOpen])

	if (!container) return null

	return createPortal(
		<div className={cn("pointer-events-none absolute inset-x-0 top-0 z-20", CONTEXT_USAGE_GUTTER_CLASS)}>
			<div
				data-context-usage-panel=""
				role="dialog"
				aria-label="Context usage"
				className={cn(
					widthClass,
					"pointer-events-auto rounded-md bg-popover text-[13px] shadow-md ring-1 ring-foreground/10",
				)}
			>
				<div className="space-y-3 p-3">
					<div className="space-y-1.5">
						<div className="flex items-start justify-between gap-3">
							<p className="pt-0.5 font-medium text-foreground/80">Context usage</p>
							<button
								type="button"
								aria-label="Close"
								onClick={() => setOpen(false)}
								className="grid size-7 shrink-0 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
							>
								<XIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
							</button>
						</div>
						<div className="flex items-center justify-between tabular-nums text-muted-foreground">
							<span>
								{formatTokens(used)} / {formatTokens(windowTokens)}
							</span>
							<span>{percent}%</span>
						</div>
						<div className="flex h-1.5 overflow-hidden rounded-full bg-muted">
							{filledRows.length > 0
								? filledRows.map((row) => (
										<div
											key={row.id}
											className={cn("h-full", CATEGORY_COLORS[row.id])}
											style={{
												width: `${windowTokens > 0 ? (row.tokens / windowTokens) * 100 : 0}%`,
											}}
										/>
									))
								: percent > 0 && (
										<div
											className="h-full rounded-full bg-foreground/70"
											style={{ width: `${percent}%` }}
										/>
									)}
						</div>
					</div>

					<div>
						<p className="mb-1.5 font-medium text-foreground/80">Prompt breakdown</p>
						<div className="space-y-1 text-muted-foreground">
							{rows.map((row) => (
								<div key={row.id} className="flex items-center justify-between gap-3">
									<span className="inline-flex min-w-0 items-center gap-1.5">
										<span
											className={cn("size-1.5 shrink-0 rounded-full", CATEGORY_COLORS[row.id])}
											aria-hidden="true"
										/>
										<span className="truncate">{row.label}</span>
									</span>
									<span className="shrink-0 text-right tabular-nums">
										{formatTokens(row.tokens)}
										<span className="ml-2 text-muted-foreground/70">{row.sharePercent}%</span>
									</span>
								</div>
							))}
						</div>
					</div>
				</div>
			</div>
		</div>,
		container,
	)
}
