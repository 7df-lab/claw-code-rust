import { cn } from "@devo/ui/lib/utils"
import {
	CirclePauseIcon,
	CirclePlayIcon,
	CornerDownRightIcon,
	GoalIcon,
	GripVerticalIcon,
	Loader2Icon,
	PencilIcon,
	Trash2Icon,
	XIcon,
} from "lucide-react"
import { useEffect, useState, type DragEvent, type ReactNode } from "react"
import { formatWorkDuration } from "../../lib/session-metrics"
import { queueRenderPreview } from "../../lib/queue-helpers"

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
	draggingQueueItemId?: string | null
	onEditGoal?: () => void
	onPauseGoal?: () => void
	onResumeGoal?: () => void
	onClearGoal?: () => void
	onSteerQueueItem?: (item: ComposerQueueItem) => void
	onEditQueueItem?: (item: ComposerQueueItem) => void
	onRemoveQueueItem?: (item: ComposerQueueItem) => void
	onReorderQueueItem?: (fromIndex: number, toIndex: number) => void
	onQueueDragStart?: (itemId: string) => void
	onQueueDragEnd?: () => void
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
	destructive,
	onClick,
	children,
}: {
	label: string
	disabled?: boolean
	active?: boolean
	destructive?: boolean
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
				destructive && "hover:text-destructive",
			)}
		>
			{children}
		</button>
	)
}

function queueItemBusy(status: ComposerQueueItemStatus): boolean {
	return status === "submitting" || status === "steering" || status === "removing"
}

interface QueueItemRowProps {
	item: ComposerQueueItem
	index: number
	dragging?: boolean
	dragOver?: boolean
	onSteer?: (item: ComposerQueueItem) => void
	onEdit?: (item: ComposerQueueItem) => void
	onRemove?: (item: ComposerQueueItem) => void
	onDragStart?: (index: number) => void
	onDragEnter?: (index: number) => void
	onDragEnd?: () => void
	onDrop?: (index: number) => void
}

function QueueItemRow({
	item,
	index,
	dragging,
	dragOver,
	onSteer,
	onEdit,
	onRemove,
	onDragStart,
	onDragEnter,
	onDragEnd,
	onDrop,
}: QueueItemRowProps) {
	const busy = queueItemBusy(item.status)
	const canAct = item.status === "queued"
	const canEdit = !busy && (item.fileCount ?? 0) === 0
	const preview = queueRenderPreview(item.text)

	const handleDragStart = (event: DragEvent<HTMLDivElement>) => {
		if (!canAct) {
			event.preventDefault()
			return
		}
		event.dataTransfer.effectAllowed = "move"
		event.dataTransfer.setData("text/plain", item.id)
		onDragStart?.(index)
	}

	const handleDragOver = (event: DragEvent<HTMLDivElement>) => {
		if (!canAct) return
		event.preventDefault()
		event.dataTransfer.dropEffect = "move"
		onDragEnter?.(index)
	}

	const handleDrop = (event: DragEvent<HTMLDivElement>) => {
		event.preventDefault()
		onDrop?.(index)
	}

	return (
		<div
			draggable={canAct}
			onDragStart={handleDragStart}
			onDragOver={handleDragOver}
			onDragEnter={handleDragOver}
			onDragEnd={() => onDragEnd?.()}
			onDrop={handleDrop}
			className={cn(
				"group/queue-row flex min-h-9 items-center gap-2 px-3 text-sm text-muted-foreground transition-colors hover:bg-muted/30",
				dragging && "scale-[1.01] bg-muted/50 shadow-sm",
				dragOver && !dragging && "bg-muted/20",
			)}
		>
			<GripVerticalIcon
				className={cn(
					"size-3.5 shrink-0 cursor-grab stroke-[1.5] text-muted-foreground/40 active:cursor-grabbing",
					!canAct && "opacity-30",
				)}
			/>
			<div className="min-w-0 flex flex-1 items-center gap-1.5">
				<span className="truncate">{preview || "(empty)"}</span>
				{item.fileCount ? (
					<span className="shrink-0 rounded bg-muted px-1.5 text-[11px] text-muted-foreground">
						{item.fileCount} file{item.fileCount === 1 ? "" : "s"}
					</span>
				) : null}
				{item.status === "error" && item.error ? (
					<span className="shrink-0 text-[11px] text-destructive">{item.error}</span>
				) : null}
			</div>
			<div className="flex shrink-0 items-center gap-0.5">
				<button
					type="button"
					disabled={!canAct || !onSteer}
					onClick={() => onSteer?.(item)}
					className="inline-flex h-7 shrink-0 items-center gap-1 rounded-md px-2 text-[12px] font-medium text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-50"
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
					destructive
					onClick={() => onRemove?.(item)}
				>
					{item.status === "removing" ? (
						<Loader2Icon className="size-3.5 animate-spin stroke-[1.5]" />
					) : (
						<Trash2Icon className="size-3.5 stroke-[1.5]" />
					)}
				</RowIconButton>
			</div>
		</div>
	)
}

interface ComposerQueueListProps {
	items: ComposerQueueItem[]
	draggingQueueItemId?: string | null
	onSteer?: (item: ComposerQueueItem) => void
	onEdit?: (item: ComposerQueueItem) => void
	onRemove?: (item: ComposerQueueItem) => void
	onReorder?: (fromIndex: number, toIndex: number) => void
	onDragStart?: (itemId: string) => void
	onDragEnd?: () => void
}

function ComposerQueueList({
	items,
	draggingQueueItemId,
	onSteer,
	onEdit,
	onRemove,
	onReorder,
	onDragStart,
	onDragEnd,
}: ComposerQueueListProps) {
	const [dragIndex, setDragIndex] = useState<number | null>(null)
	const [hoverIndex, setHoverIndex] = useState<number | null>(null)

	if (items.length === 0) return null

	return (
		<>
			{items.map((item, index) => (
				<QueueItemRow
					key={item.id}
					item={item}
					index={index}
					dragging={draggingQueueItemId === item.id}
					dragOver={hoverIndex === index && dragIndex !== null && dragIndex !== index}
					onSteer={onSteer}
					onEdit={onEdit}
					onRemove={onRemove}
					onDragStart={(nextIndex) => {
						setDragIndex(nextIndex)
						onDragStart?.(item.id)
					}}
					onDragEnter={setHoverIndex}
					onDragEnd={() => {
						setDragIndex(null)
						setHoverIndex(null)
						onDragEnd?.()
					}}
					onDrop={(toIndex) => {
						if (dragIndex !== null) onReorder?.(dragIndex, toIndex)
						setDragIndex(null)
						setHoverIndex(null)
						onDragEnd?.()
					}}
				/>
			))}
		</>
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
	draggingQueueItemId = null,
	onEditGoal,
	onPauseGoal,
	onResumeGoal,
	onClearGoal,
	onSteerQueueItem,
	onEditQueueItem,
	onRemoveQueueItem,
	onReorderQueueItem,
	onQueueDragStart,
	onQueueDragEnd,
}: ComposerStatusStackProps) {
	if (!goal && queueItems.length === 0) return null

	return (
		// User requirement: reuse this composer-adjacent strip for goal state
		// and queued follow-up rows instead of scattering status below messages.
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
				<ComposerQueueList
					items={queueItems}
					draggingQueueItemId={draggingQueueItemId}
					onSteer={onSteerQueueItem}
					onEdit={onEditQueueItem}
					onRemove={onRemoveQueueItem}
					onReorder={onReorderQueueItem}
					onDragStart={onQueueDragStart}
					onDragEnd={onQueueDragEnd}
				/>
			</div>
		</div>
	)
}
