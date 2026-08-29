/**
 * Session-header context occupancy control.
 *
 * Replaces the previous stats overview button with a circular progress ring
 * for window fill. Clicking opens the same category breakdown as TUI `/status`.
 */
import { Popover, PopoverContent, PopoverTrigger } from "@devo/ui/components/popover"
import { cn } from "@devo/ui/lib/utils"
import { useAtomValue } from "jotai"
import { useEffect, useMemo } from "react"
import { sessionNativeFamily } from "../atoms/session-native"
import {
	occupancyCategoryRows,
	windowFillPercent,
	type ContextCategoryId,
} from "../lib/context-occupancy"
import { formatTokens } from "../lib/session-metrics"
import { getBaseClient, getProjectClient } from "../services/connection-manager"

const CATEGORY_COLORS: Record<ContextCategoryId, string> = {
	base: "bg-muted-foreground/45",
	skills: "bg-chart-1",
	toolsBuiltin: "bg-chart-2",
	toolsMcp: "bg-chart-4",
	conversation: "bg-chart-3",
}

interface ContextUsageButtonProps {
	sessionId: string
	directory?: string
}

export function ContextUsageButton({ sessionId, directory }: ContextUsageButtonProps) {
	const native = useAtomValue(sessionNativeFamily(sessionId))
	const occupancy = native.occupancy
	const used = occupancy?.totalTokens ?? Number(native.usage?.used ?? 0)
	const windowTokens = occupancy?.contextWindowTokens ?? Number(native.usage?.size ?? 0)
	const percent = windowFillPercent(used, windowTokens)
	const rows = useMemo(() => occupancyCategoryRows(occupancy), [occupancy])
	const filledRows = rows.filter((row) => row.tokens > 0)
	const strokeClass =
		percent >= 90 ? "text-red-400" : percent >= 70 ? "text-yellow-400" : "text-muted-foreground"

	useEffect(() => {
		if (occupancy) return
		const client = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
		if (!client?.context?.usage?.read) return
		void client.context.usage.read({ sessionID: sessionId }).catch(() => {})
	}, [directory, occupancy, sessionId])

	const size = 14
	const strokeWidth = 2.5
	const radius = (size - strokeWidth) / 2
	const circumference = 2 * Math.PI * radius
	const offset = circumference - (Math.min(percent, 100) / 100) * circumference

	return (
		<Popover>
			<PopoverTrigger
				render={
					<button
						type="button"
						aria-label={`Context usage ${percent}%`}
						className={cn(
							"inline-flex size-8 items-center justify-center rounded-md transition-colors hover:bg-muted hover:text-foreground",
							strokeClass,
						)}
					/>
				}
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
			</PopoverTrigger>
			<PopoverContent side="bottom" align="end" sideOffset={8} className="w-64 gap-0 p-0">
				<div className="space-y-3 p-3 text-xs">
					<div className="space-y-1.5">
						<p className="font-medium text-foreground/80">Context usage</p>
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
			</PopoverContent>
		</Popover>
	)
}
