/**
 * Compact session metrics bar for the agent-detail app bar.
 *
 * Inline: work time, cost, and error/retry alerts.
 * Popover: full token breakdown, exchanges, model distribution,
 * tool calls, and cache efficiency.
 *
 * Context window occupancy is the circular progress control in the session
 * header (see context-usage-button.tsx). The composer status bar also shows
 * a compact usage indicator (see prompt-toolbar.tsx).
 */
import { Popover, PopoverContent, PopoverTrigger } from "@devo/ui/components/popover"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { useAtomValue } from "jotai"
import { BarChart3Icon, CoinsIcon, TimerIcon } from "lucide-react"
import { Fragment, memo, useCallback, useEffect, useRef, useState } from "react"
import { type SessionMetricsValue, sessionMetricsFamily } from "../atoms/derived/session-metrics"
import type { ChatTurn } from "../hooks/use-session-chat"
import { computeLatestTurnTimerSplit, formatTokens, formatWorkDuration } from "../lib/session-metrics"

// ============================================================
// Tool category display labels
// ============================================================

const TOOL_CATEGORY_LABELS: Record<string, string> = {
	explore: "Read/Search",
	edit: "Edit/Write",
	run: "Run",
	delegate: "Agent",
	plan: "Plan",
	ask: "Ask",
	fetch: "Fetch",
	other: "Other",
}

// ============================================================
// SessionMetricsBar
// ============================================================

interface SessionMetricsBarProps {
	sessionId: string
	turns: ChatTurn[]
	isWorking: boolean
}

interface SessionMetricsOverviewButtonProps {
	sessionId: string
}

/**
 * Compact metrics bar that reads from `sessionMetricsFamily` directly.
 * Only re-renders when the session's metrics change (structural equality).
 *
 * Shows time + cost inline, with a popover for the full stats breakdown.
 */
export const SessionMetricsBar = memo(function SessionMetricsBar({
	sessionId,
	turns,
	isWorking,
}: SessionMetricsBarProps) {
	const metrics = useAtomValue(sessionMetricsFamily(sessionId))
	const lastTurn = turns.at(-1)
	const lastLiveElapsedRef = useRef<{ turnId: string; elapsedMs: number } | null>(null)
	const lastLiveElapsed = lastLiveElapsedRef.current
	const fallbackCompletedMs =
		!isWorking && lastTurn && lastLiveElapsed && lastTurn.id === lastLiveElapsed.turnId
			? lastLiveElapsed.elapsedMs
			: undefined
	const turnTimer = computeLatestTurnTimerSplit(turns, {
		mode: isWorking ? "running" : "stopped",
		fallbackCompletedMs,
	})
	const handleLiveElapsedChange = useCallback(
		(elapsedMs: number) => {
			if (!lastTurn) return
			lastLiveElapsedRef.current = { turnId: lastTurn.id, elapsedMs }
		},
		[lastTurn],
	)

	return (
		<div className="flex min-w-0 flex-wrap items-center gap-x-1.5 gap-y-0.5">
			{/* Current turn time */}
			<Tooltip>
				<TooltipTrigger
					render={
						<span className="inline-flex items-center gap-1 text-xs tabular-nums text-muted-foreground/60" />
					}
				>
					<TimerIcon className="size-3" aria-hidden="true" />
					{turnTimer.activeStartMs != null ? (
						<LiveWorkTime
							completedMs={turnTimer.completedMs}
							activeStartMs={turnTimer.activeStartMs}
							onElapsedChange={handleLiveElapsedChange}
						/>
					) : (
						formatWorkDuration(turnTimer.completedMs)
					)}
				</TooltipTrigger>
				<TooltipContent side="bottom" align="end">
					<div className="space-y-1 text-xs">
						<p className="font-medium">Current Turn</p>
						<p className="text-background/60">Latest turn elapsed time</p>
					</div>
				</TooltipContent>
			</Tooltip>

			{/* Cost */}
			{metrics.costRaw > 0 && (
				<>
					<Separator />
					<Tooltip>
						<TooltipTrigger
							render={
								<span className="inline-flex items-center gap-1 text-xs tabular-nums text-muted-foreground/60" />
							}
						>
							<CoinsIcon className="size-3" aria-hidden="true" />
							{metrics.cost}
						</TooltipTrigger>
						<TooltipContent side="bottom" align="end">
							<div className="space-y-1 text-xs">
								<p className="font-medium">Cost</p>
								<p className="text-background/60">Avg per exchange: {metrics.avgExchangeCost}</p>
							</div>
						</TooltipContent>
					</Tooltip>
				</>
			)}

		{/* Details popover -- full stats breakdown */}
			{(metrics.tokensRaw > 0 || metrics.toolCallCount > 0) && (
				<>
					<Separator />
					<MetricsPopover metrics={metrics} />
				</>
			)}
		</div>
	)
})

export function SessionMetricsOverviewButton({ sessionId }: SessionMetricsOverviewButtonProps) {
	const metrics = useAtomValue(sessionMetricsFamily(sessionId))

	return (
		<MetricsPopover
			metrics={metrics}
			ariaLabel="Show session overview"
			triggerClassName="inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground"
			triggerIconClassName="size-4"
		/>
	)
}

// ============================================================
// Metrics detail popover
// ============================================================

function MetricsPopover({
	metrics,
	ariaLabel = "Show session metrics",
	triggerClassName = "inline-flex items-center gap-1 text-xs text-muted-foreground/60 transition-colors hover:text-muted-foreground",
	triggerIconClassName = "size-3",
}: {
	metrics: SessionMetricsValue
	ariaLabel?: string
	triggerClassName?: string
	triggerIconClassName?: string
}) {
	const { raw } = metrics

	return (
		<Popover>
			<PopoverTrigger
				render={
					<button
						type="button"
						aria-label={ariaLabel}
						className={triggerClassName}
					/>
				}
			>
				<BarChart3Icon className={triggerIconClassName} aria-hidden="true" />
			</PopoverTrigger>
			<PopoverContent side="bottom" align="end" sideOffset={8} className="w-64 gap-0 p-0">
				<div className="space-y-3 p-3 text-xs">
					{/* Summary row */}
					<div className="grid grid-cols-3 gap-2">
						<MetricCell label="Time" value={metrics.workTime} />
						<MetricCell label="Cost" value={metrics.cost} />
						<MetricCell label="Tokens" value={metrics.tokens} />
					</div>

					{/* Token breakdown */}
					{metrics.tokensRaw > 0 && (
						<div>
							<p className="mb-1.5 font-medium text-foreground/80">Token Breakdown</p>
							<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-muted-foreground">
								<span>Input</span>
								<span className="text-right tabular-nums">{formatTokens(raw.tokens.input)}</span>
								<span>Output</span>
								<span className="text-right tabular-nums">{formatTokens(raw.tokens.output)}</span>
								{raw.tokens.reasoning > 0 && (
									<>
										<span>Reasoning</span>
										<span className="text-right tabular-nums">
											{formatTokens(raw.tokens.reasoning)}
										</span>
									</>
								)}
								<span>Cache read</span>
								<span className="text-right tabular-nums">
									{formatTokens(raw.tokens.cacheRead)}
								</span>
								{raw.tokens.cacheWrite > 0 && (
									<>
										<span>Cache write</span>
										<span className="text-right tabular-nums">
											{formatTokens(raw.tokens.cacheWrite)}
										</span>
									</>
								)}
							</div>
							{metrics.cacheEfficiency > 0 && (
								<p className="mt-1 text-muted-foreground">
									Cache hit rate: {metrics.cacheEfficiencyFormatted}
								</p>
							)}
						</div>
					)}

					{/* Exchanges + model distribution */}
					<div>
						<p className="mb-1.5 font-medium text-foreground/80">Exchanges</p>
						<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-muted-foreground">
							<span>User messages</span>
							<span className="text-right tabular-nums">{metrics.userMessageCount}</span>
							<span>Agent responses</span>
							<span className="text-right tabular-nums">{metrics.assistantMessageCount}</span>
						</div>
						{metrics.modelDistributionDisplay.length > 0 && (
							<div className="mt-1.5 border-t border-border/50 pt-1.5">
								<p className="mb-0.5 font-medium text-foreground/80">Models</p>
								<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-muted-foreground">
									{metrics.modelDistributionDisplay.map(({ name, count }) => (
										<Fragment key={name}>
											<span>{name}</span>
											<span className="text-right tabular-nums">{count}</span>
										</Fragment>
									))}
								</div>
							</div>
						)}
					</div>

					{/* Tool calls */}
					{metrics.toolCallCount > 0 && (
						<div>
							<p className="mb-1.5 font-medium text-foreground/80">
								Tool Calls ({metrics.toolCallCount})
							</p>
							<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-muted-foreground">
								{Object.entries(metrics.toolBreakdown)
									.sort(([, a], [, b]) => (Number(b) || 0) - (Number(a) || 0))
									.map(([cat, count]) => (
										<Fragment key={cat}>
											<span>{TOOL_CATEGORY_LABELS[cat] ?? cat}</span>
											<span className="text-right tabular-nums">{count}</span>
										</Fragment>
									))}
							</div>
						</div>
					)}

					{/* Averages */}
					<div className="border-t border-border/50 pt-2">
						<div className="grid grid-cols-[auto_1fr] gap-x-3 gap-y-0.5 text-muted-foreground">
							<span>Avg cost / exchange</span>
							<span className="text-right tabular-nums">{metrics.avgExchangeCost}</span>
							<span>Avg time / exchange</span>
							<span className="text-right tabular-nums">{metrics.avgExchangeTime}</span>
						</div>
					</div>
				</div>
			</PopoverContent>
		</Popover>
	)
}

// ============================================================
// Small metric cell for the summary row
// ============================================================

function MetricCell({ label, value }: { label: string; value: string }) {
	return (
		<div className="flex flex-col items-center gap-0.5">
			<span className="text-[10px] text-muted-foreground">{label}</span>
			<span className="tabular-nums text-foreground">{value}</span>
		</div>
	)
}

// ============================================================
// Live work-time ticker -- updates every second while active
// ============================================================

/**
 * Tiny leaf component that ticks every second to show live work time.
 * Only mounts when the agent has an in-progress message, so the rest of
 * the metrics bar is not affected by the interval.
 */
function LiveWorkTime({
	completedMs,
	activeStartMs,
	onElapsedChange,
}: {
	completedMs: number
	activeStartMs: number
	onElapsedChange?: (elapsedMs: number) => void
}) {
	const [display, setDisplay] = useState(() =>
		formatWorkDuration(Math.max(0, completedMs + (Date.now() - activeStartMs))),
	)

	useEffect(() => {
		const tick = () => {
			const elapsedMs = Math.max(0, completedMs + (Date.now() - activeStartMs))
			onElapsedChange?.(elapsedMs)
			setDisplay(formatWorkDuration(elapsedMs))
		}
		tick()
		const id = setInterval(tick, 1_000)
		return () => clearInterval(id)
	}, [completedMs, activeStartMs, onElapsedChange])

	return <>{display}</>
}

// ============================================================
// Small separator dot
// ============================================================

function Separator() {
	return (
		<span className="text-muted-foreground/20" aria-hidden="true">
			·
		</span>
	)
}
