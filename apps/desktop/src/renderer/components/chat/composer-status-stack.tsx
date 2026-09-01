import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuTrigger,
} from "@devo/ui/components/dropdown-menu"
import { cn } from "@devo/ui/lib/utils"
import {
	CirclePauseIcon,
	CirclePlayIcon,
	CornerDownRightIcon,
	GoalIcon,
	GripVerticalIcon,
	Loader2Icon,
	MoreHorizontalIcon,
	PencilIcon,
	Trash2Icon,
	XIcon,
} from "lucide-react"
import { useEffect, useState, type ReactNode } from "react"
import { formatWorkDuration } from "../../lib/session-metrics"

export type ComposerGoalStatus = "active" | "paused" | "budgetLimited" | "complete"
export type ComposerQueueItemStatus = "submitting" | "queued" | "steering" | "removing" | "error"

export interface ComposerGoal {
	objective: string
	status: ComposerGoalStatus
	timeUsedSeconds?: number | string | bigint | null
	observedAtMs?: number
}

export interface ComposerQueueItem {
	id: string
	text: string
	status: ComposerQueueItemStatus
	activeTurnId?: string
	queuedInputId?: string
	fileCount?: number
	createdAtMs?: number
	error?: string
}

interface ComposerStatusStackProps {
	goal?: ComposerGoal | null
	goalAction?: "edit" | "pause" | "resume" | "clear" | null
	queueItems?: ComposerQueueItem[]
	onEditGoal?: () => void
	onPauseGoal?: () => void
	onResumeGoal?: () => void
	onClearGoal?: () => void
	onSteerQueueItem?: (item: ComposerQueueItem) => void
	onEditQueueItem?: (item: ComposerQueueItem) => void
	onRemoveQueueItem?: (item: ComposerQueueItem) => void
}

function protocolNumber(value: number | string | bigint | null | undefined): number {
	if (typeof value === "number") return Number.isFinite(value) ? value : 0
	if (typeof value === "bigint") return Number(value)
	if (typeof value === "string") {
		const parsed = Number(value)
		return Number.isFinite(parsed) ? parsed : 0
	}
	return 0
}

function goalStatusLabel(status: ComposerGoalStatus): string {
	switch (status) {
		case "active":
			return "Pursuing goal"
		case "paused":
			return "Goal paused"
		case "budgetLimited":
			return "Goal budget reached"
		case "complete":
			return "Goal complete"
	}
}

function GoalElapsed({ goal }: { goal: ComposerGoal }) {
	const [now, setNow] = useState(() => Date.now())

	useEffect(() => {
		if (goal.status !== "active") return
		const timer = setInterval(() => setNow(Date.now()), 1_000)
		return () => clearInterval(timer)
	}, [goal.status])

	const observedAt = goal.observedAtMs ?? now
	const liveDeltaMs = goal.status === "active" ? Math.max(0, now - observedAt) : 0
	const elapsedMs = protocolNumber(goal.timeUsedSeconds) * 1_000 + liveDeltaMs
	if (elapsedMs < 1_000) return null

	return (
		<span className="shrink-0 tabular-nums text-muted-foreground/80">
			{formatWorkDuration(elapsedMs)}
		</span>
	)
}

function RowIconButton({
	label,
	disabled,
	active,
	onClick,
	children,
}: {
	label: string
	disabled?: boolean
	active?: boolean
	onClick?: () => void
	children: ReactNode
}) {
	return (
		<button
			type="button"
			aria-label={label}
			title={label}
			disabled={disabled}
			onClick={onClick}
			className={cn(
				"grid size-7 shrink-0 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-50",
				active && "bg-muted text-foreground",
			)}
		>
			{children}
		</button>
	)
}

function queueItemBusy(status: ComposerQueueItemStatus): boolean {
	return status === "submitting" || status === "steering" || status === "removing"
}

function queueItemLabel(item: ComposerQueueItem): string {
	switch (item.status) {
		case "submitting":
			return "Queueing"
		case "steering":
			return "Steering"
		case "removing":
			return "Removing"
		case "error":
			return "Queue failed"
		case "queued":
			return "Queued"
	}
}

interface QueueItemRowProps {
	item: ComposerQueueItem
	onSteer?: (item: ComposerQueueItem) => void
	onEdit?: (item: ComposerQueueItem) => void
	onRemove?: (item: ComposerQueueItem) => void
}

function QueueItemRow({ item, onSteer, onEdit, onRemove }: QueueItemRowProps) {
	const busy = queueItemBusy(item.status)
	const canAct = item.status === "queued"
	const canEdit = !busy && (item.fileCount ?? 0) === 0
	const label = queueItemLabel(item)
	const detail = item.fileCount ? `${item.text} (${item.fileCount} file${item.fileCount === 1 ? "" : "s"})` : item.text

	return (
		<div className="group/queue-row flex min-h-10 items-center gap-2 px-3 text-sm text-muted-foreground">
			<GripVerticalIcon className="size-3 shrink-0 stroke-[1.5] text-muted-foreground/35 opacity-0 transition-opacity group-hover/queue-row:opacity-100" />
			<CornerDownRightIcon className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground/70" />
			<div className="min-w-0 flex flex-1 items-center gap-1.5">
				<span className="truncate">{detail}</span>
				{item.status === "error" && item.error ? (
					<span className="shrink-0 text-[11px] text-destructive">{item.error}</span>
				) : null}
			</div>
			<span className="shrink-0 text-[12px] text-muted-foreground/70">{label}</span>
			<button
				type="button"
				disabled={!canAct || !onSteer}
				onClick={() => onSteer?.(item)}
				className="inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2 text-[12px] font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
			>
				{item.status === "steering" ? (
					<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
				) : (
					<CornerDownRightIcon className="size-3.5 stroke-[1.5]" />
				)}
				Steer
			</button>
			<RowIconButton
				label="Edit queued message"
				disabled={!canEdit || !onEdit}
				onClick={() => onEdit?.(item)}
			>
				<PencilIcon className="size-3.5 stroke-[1.5]" />
			</RowIconButton>
			<RowIconButton
				label="Remove queued message"
				disabled={busy || !onRemove}
				active={item.status === "removing"}
				onClick={() => onRemove?.(item)}
			>
				{item.status === "removing" ? (
					<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
				) : (
					<Trash2Icon className="size-3.5 stroke-[1.5]" />
				)}
			</RowIconButton>
			<DropdownMenu>
				<DropdownMenuTrigger
					render={
						<button
							type="button"
							aria-label="Queued message options"
							disabled={busy}
							className="grid size-7 shrink-0 place-items-center rounded-md text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
						/>
					}
				>
					<MoreHorizontalIcon className="size-3.5 stroke-[1.5]" />
				</DropdownMenuTrigger>
				<DropdownMenuContent align="end" side="top" className="min-w-[190px]">
					<DropdownMenuItem
						disabled={!canEdit || !onEdit}
						onClick={() => onEdit?.(item)}
						className="flex items-center gap-2"
					>
						<PencilIcon className="size-3.5 stroke-[1.5]" />
						<span>Edit message</span>
					</DropdownMenuItem>
					<DropdownMenuItem
						disabled={!canAct || !onSteer}
						onClick={() => onSteer?.(item)}
						className="flex items-center gap-2"
					>
						<CornerDownRightIcon className="size-3.5 stroke-[1.5]" />
						<span>Turn off queueing</span>
					</DropdownMenuItem>
				</DropdownMenuContent>
			</DropdownMenu>
		</div>
	)
}

interface ActiveGoalRowProps {
	goal: ComposerGoal
	goalAction?: ComposerStatusStackProps["goalAction"]
	onEditGoal?: () => void
	onPauseGoal?: () => void
	onResumeGoal?: () => void
	onClearGoal?: () => void
}

function ActiveGoalRow({
	goal,
	goalAction = null,
	onEditGoal,
	onPauseGoal,
	onResumeGoal,
	onClearGoal,
}: ActiveGoalRowProps) {
	const statusLabel = goalStatusLabel(goal.status)
	const isPaused = goal.status === "paused" || goal.status === "budgetLimited"
	const toggleLabel = isPaused ? "Resume goal" : "Pause goal"
	const toggleAction = isPaused ? onResumeGoal : onPauseGoal

	return (
		<div className="flex min-h-8 items-center gap-2 px-3 py-1.5 text-sm text-muted-foreground">
			<GoalIcon className="size-3.5 shrink-0 stroke-[1.5] text-muted-foreground/75" />
			<div className="min-w-0 flex flex-1 items-center gap-1.5">
				<span className="shrink-0 font-medium text-foreground">{statusLabel}</span>
				<span className="truncate">{goal.objective}</span>
			</div>
			<GoalElapsed goal={goal} />
			<div className="flex shrink-0 items-center gap-0.5">
				<RowIconButton
					label="Edit goal"
					disabled={goalAction !== null}
					active={goalAction === "edit"}
					onClick={onEditGoal}
				>
					{goalAction === "edit" ? (
						<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
					) : (
						<PencilIcon className="size-3.5 stroke-[1.5]" />
					)}
				</RowIconButton>
				<RowIconButton
					label={toggleLabel}
					disabled={goalAction !== null}
					active={goalAction === "pause" || goalAction === "resume"}
					onClick={toggleAction}
				>
					{goalAction === "pause" || goalAction === "resume" ? (
						<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
					) : isPaused ? (
						<CirclePlayIcon className="size-3.5 stroke-[1.5]" />
					) : (
						<CirclePauseIcon className="size-3.5 stroke-[1.5]" />
					)}
				</RowIconButton>
				<RowIconButton
					label="Cancel goal"
					disabled={goalAction !== null}
					active={goalAction === "clear"}
					onClick={onClearGoal}
				>
					{goalAction === "clear" ? (
						<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
					) : (
						<XIcon className="size-3.5 stroke-[1.5]" />
					)}
				</RowIconButton>
			</div>
		</div>
	)
}

export function ComposerStatusStack({
	goal,
	goalAction = null,
	queueItems = [],
	onEditGoal,
	onPauseGoal,
	onResumeGoal,
	onClearGoal,
	onSteerQueueItem,
	onEditQueueItem,
	onRemoveQueueItem,
}: ComposerStatusStackProps) {
	if (!goal && queueItems.length === 0) return null

	return (
		// User requirement: reuse this composer-adjacent strip for goal state
		// and future queued follow-up rows instead of scattering status below messages.
		<div className="order-first w-full overflow-hidden border-b border-border/50">
			<div className="divide-y divide-border/50">
				{goal && (
					<ActiveGoalRow
						goal={goal}
						goalAction={goalAction}
						onEditGoal={onEditGoal}
						onPauseGoal={onPauseGoal}
						onResumeGoal={onResumeGoal}
						onClearGoal={onClearGoal}
					/>
				)}
				{queueItems.map((item) => (
					<QueueItemRow
						key={item.id}
						item={item}
						onSteer={onSteerQueueItem}
						onEdit={onEditQueueItem}
						onRemove={onRemoveQueueItem}
					/>
				))}
			</div>
		</div>
	)
}
