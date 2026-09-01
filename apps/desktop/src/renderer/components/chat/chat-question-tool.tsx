import { cn } from "@devo/ui/lib/utils"
import { CheckIcon } from "lucide-react"
import type { ToolPart } from "../../lib/types"

export type QuestionToolEntry = {
	id: string
	header: string
	question: string
	isSecret: boolean
	answer: string | null
}

export function isQuestionToolName(tool: string): boolean {
	return tool === "question" || tool === "request_user_input"
}

export function isQuestionToolInput(
	tool: string,
	input?: Record<string, unknown> | null,
): boolean {
	if (isQuestionToolName(tool)) return true
	return Array.isArray(input?.questions)
}

export function parseQuestionToolEntries(part: {
	tool: string
	state: {
		input?: Record<string, unknown>
		output?: string
		status?: string
	}
}): QuestionToolEntry[] {
	const questions = part.state.input?.questions
	if (!Array.isArray(questions)) return []
	const answers = parseQuestionAnswerMap(part.state.output)
	return questions.map((raw, index) => {
		const record = raw && typeof raw === "object" ? (raw as Record<string, unknown>) : {}
		const id = typeof record.id === "string" && record.id ? record.id : `question-${index}`
		const header = typeof record.header === "string" ? record.header : ""
		const question = typeof record.question === "string" ? record.question : ""
		const isSecret = record.isSecret === true || record.is_secret === true
		const values = answers[id] ?? []
		const first = values.map((value) => value.trim()).find(Boolean) ?? null
		return {
			id,
			header,
			question,
			isSecret,
			answer: first ? (isSecret ? "••••••" : first) : null,
		}
	})
}

export function questionToolSubtitle(part: {
	tool: string
	state: {
		status?: string
		input?: Record<string, unknown>
		output?: string
		raw?: string
	}
}): string | undefined {
	const entries = parseQuestionToolEntries(part)
	if (entries.length === 1) {
		return entries[0]?.question || entries[0]?.header || undefined
	}
	if (entries.length > 1) {
		return `${entries.length} questions`
	}
	if (part.state.status === "pending" || part.state.status === "running") {
		return "Asking a question…"
	}
	return undefined
}

function parseQuestionAnswerMap(output: string | undefined): Record<string, string[]> {
	if (!output) return {}
	let value: unknown = output
	try {
		value = JSON.parse(output) as unknown
	} catch {
		return {}
	}
	if (!value || typeof value !== "object" || Array.isArray(value)) return {}
	const record = value as Record<string, unknown>
	const nested =
		record.answers && typeof record.answers === "object" && !Array.isArray(record.answers)
			? (record.answers as Record<string, unknown>)
			: record
	const mapped: Record<string, string[]> = {}
	for (const [id, entry] of Object.entries(nested)) {
		if (typeof entry === "string") {
			mapped[id] = [entry]
			continue
		}
		if (Array.isArray(entry)) {
			mapped[id] = entry.map(String)
			continue
		}
		if (entry && typeof entry === "object" && Array.isArray((entry as { answers?: unknown }).answers)) {
			mapped[id] = ((entry as { answers: unknown[] }).answers).map(String)
		}
	}
	return mapped
}

export function QuestionToolContent({ part }: { part: ToolPart }) {
	const entries = parseQuestionToolEntries(part)
	if (entries.length === 0) return null
	const waiting = part.state.status === "pending" || part.state.status === "running"

	return (
		<div className="space-y-2.5 px-3.5 py-2">
			{entries.map((entry) => (
				<div key={entry.id} className="min-w-0">
					{entry.header ? (
						<div className="text-[13px] font-medium text-muted-foreground">{entry.header}</div>
					) : null}
					{entry.question ? (
						<div
							className={cn(
								"text-[13px] leading-5 text-foreground",
								entry.header && "mt-0.5",
							)}
						>
							{entry.question}
						</div>
					) : null}
					<div className="mt-1 flex items-start gap-1.5 text-[13px] leading-5">
						{entry.answer ? (
							<CheckIcon
								className="mt-0.5 size-3.5 shrink-0 stroke-[1.5] text-muted-foreground"
								aria-hidden="true"
							/>
						) : null}
						<span
							className={
								entry.answer ? "text-foreground" : "text-muted-foreground/70"
							}
						>
							{entry.answer ?? (waiting ? "Waiting for a reply" : "No answer")}
						</span>
					</div>
				</div>
			))}
		</div>
	)
}
