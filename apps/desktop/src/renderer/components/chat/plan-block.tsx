import { Message, MessageContent, MessageResponse } from "@devo/ui/components/ai-elements/message"
import { Button } from "@devo/ui/components/button"
import { cn } from "@devo/ui/lib/utils"
import {
	CheckCircle2Icon,
	CircleDotIcon,
	ListTodoIcon,
	Loader2Icon,
	XCircleIcon,
} from "lucide-react"
import {
	TranscriptDisclosure,
	TranscriptDisclosureContent,
	TranscriptDisclosureTrigger,
} from "./transcript-disclosure"

export const DEVO_PLAN_ITEM_KIND = "plan"
export const DEVO_PROPOSED_PLAN_ITEM_KIND = "proposed_plan"

export interface PlanEntry {
	content: string
	status: string
}

export function isPlanTextPart(metadata: Record<string, unknown> | undefined): boolean {
	const kind = metadata?.["devo/itemKind"]
	return kind === DEVO_PLAN_ITEM_KIND || kind === DEVO_PROPOSED_PLAN_ITEM_KIND
}

/** Plan-mode markdown long-form proposal (Implement / Revise actions). */
export function isProposedPlanPart(metadata: Record<string, unknown> | undefined): boolean {
	return metadata?.["devo/itemKind"] === DEVO_PROPOSED_PLAN_ITEM_KIND
}

/** `update_plan` checklist — process-timeline tool cell, not final answer text. */
export function isChecklistPlanPart(metadata: Record<string, unknown> | undefined): boolean {
	return metadata?.["devo/itemKind"] === DEVO_PLAN_ITEM_KIND
}

function normalizePlanStatus(status: string): string {
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

export function planEntriesFromMetadata(metadata: Record<string, unknown> | undefined): PlanEntry[] {
	const raw = metadata?.planEntries
	if (!Array.isArray(raw)) return []
	return raw
		.map((entry) => {
			if (!entry || typeof entry !== "object") return null
			const value = entry as Record<string, unknown>
			const content = String(value.content ?? value.step ?? "").trim()
			if (!content) return null
			return {
				content,
				status: normalizePlanStatus(String(value.status ?? "pending")),
			}
		})
		.filter((entry): entry is PlanEntry => Boolean(entry?.content))
}

function PlanStepIcon({ status }: { status: string }) {
	switch (normalizePlanStatus(status)) {
		case "completed":
			return <CheckCircle2Icon className="size-3.5 text-emerald-500/80" />
		case "in_progress":
			return <Loader2Icon className="size-3.5 animate-spin text-blue-400/80" />
		case "cancelled":
			return <XCircleIcon className="size-3.5 text-muted-foreground/40" />
		default:
			return <CircleDotIcon className="size-3.5 text-muted-foreground/40" />
	}
}

function PlanChecklistBody({ entries }: { entries: PlanEntry[] }) {
	if (entries.length === 0) return null
	return (
		<ol className="space-y-1 px-1 py-1.5">
			{entries.map((entry, index) => (
				<li key={`${entry.content}-${index}`} className="flex items-start gap-2 text-xs">
					<span className="mt-0.5 shrink-0">
						<PlanStepIcon status={entry.status} />
					</span>
					<span
						className={cn(
							"min-w-0 flex-1 whitespace-pre-wrap",
							normalizePlanStatus(entry.status) === "completed" &&
								"text-muted-foreground line-through",
							normalizePlanStatus(entry.status) === "cancelled" &&
								"text-muted-foreground/70",
						)}
					>
						{entry.content}
					</span>
				</li>
			))}
		</ol>
	)
}

/**
 * Collapsible process-timeline row for `update_plan`.
 * Collapsed: "Updated plan". Expanded: the todo checklist.
 */
export function PlanChecklistRow({
	item,
	defaultOpen = false,
	open,
	onOpenChange,
}: {
	item: { text: string; metadata?: Record<string, unknown> }
	defaultOpen?: boolean
	open?: boolean
	onOpenChange?: (open: boolean) => void
}) {
	const entries = planEntriesFromMetadata(item.metadata)
	const expandable = entries.length > 0

	return (
		<TranscriptDisclosure
			defaultOpen={defaultOpen}
			expandable={expandable}
			open={open}
			onOpenChange={onOpenChange}
		>
			<TranscriptDisclosureTrigger
				label={
					<span className="inline-flex items-center gap-1.5">
						<ListTodoIcon className="size-3.5 stroke-[1.5] text-muted-foreground/70" aria-hidden="true" />
						<span>Updated plan</span>
					</span>
				}
			/>
			{expandable ? (
				<TranscriptDisclosureContent rail className="overflow-hidden">
					<PlanChecklistBody entries={entries} />
				</TranscriptDisclosureContent>
			) : null}
		</TranscriptDisclosure>
	)
}

interface PlanBlockProps {
	item: {
		text: string
		metadata?: Record<string, unknown>
	}
	showActions?: boolean
	onImplementPlan?: () => void
	onRevisePlan?: () => void
}

/** Plan-mode Proposed Plan card (markdown + optional Implement/Revise). */
export function PlanBlock({ item, showActions = false, onImplementPlan, onRevisePlan }: PlanBlockProps) {
	return (
		<div className="overflow-hidden rounded-lg border border-border bg-muted/20">
			<div className="flex items-center gap-1.5 border-b border-border/70 px-3 py-2 text-[11px] font-medium text-muted-foreground">
				<ListTodoIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
				<span>Proposed Plan</span>
			</div>
			<div className="px-3 py-2">
				<Message from="assistant">
					<MessageContent>
						<MessageResponse>{item.text}</MessageResponse>
					</MessageContent>
				</Message>
			</div>
			{showActions && (onImplementPlan || onRevisePlan) && (
				<div className="flex flex-wrap gap-2 border-t border-border/70 px-3 py-2">
					{onImplementPlan && (
						<Button size="sm" onClick={onImplementPlan}>
							Implement Plan
						</Button>
					)}
					{onRevisePlan && (
						<Button size="sm" variant="outline" onClick={onRevisePlan}>
							Revise Plan
						</Button>
					)}
				</div>
			)}
		</div>
	)
}
