import { Button } from "@devo/ui/components/button"
import { cn } from "@devo/ui/lib/utils"
import {
	ArrowRightIcon,
	ArrowUpIcon,
	CheckIcon,
	ChevronLeftIcon,
	ChevronRightIcon,
	Loader2Icon,
} from "lucide-react"
import { memo, useCallback, useEffect, useRef, useState, type ReactNode } from "react"
import type { QuestionAnswer, QuestionInfo, QuestionRequest } from "../../lib/types"

interface ChatQuestionFlowProps {
	questions: QuestionRequest[]
	onReply: (requestId: string, answers: QuestionAnswer[]) => Promise<void>
	onReject: (requestId: string) => Promise<void>
	disabled?: boolean
	isFromSubAgent?: boolean
}

function buildAnswers(
	questions: QuestionInfo[],
	selections: Map<number, Set<string>>,
	customTexts: Map<number, string>,
): QuestionAnswer[] {
	return questions.map((_, idx) => {
		const custom = (customTexts.get(idx) ?? "").trim()
		if (custom) return [custom]
		return Array.from(selections.get(idx) ?? []).slice(0, 1)
	})
}

function isQuestionAnswered(
	index: number,
	selections: Map<number, Set<string>>,
	customTexts: Map<number, string>,
): boolean {
	const selected = selections.get(index)
	const custom = (customTexts.get(index) ?? "").trim()
	return (selected && selected.size > 0) || custom.length > 0
}

function answerLabel(
	info: QuestionInfo,
	index: number,
	selections: Map<number, Set<string>>,
	customTexts: Map<number, string>,
): string | null {
	const custom = (customTexts.get(index) ?? "").trim()
	if (custom) return info.isSecret ? "••••••" : custom
	const selected = Array.from(selections.get(index) ?? [])[0]
	return selected ?? null
}

interface QuestionSectionProps {
	info: QuestionInfo
	index: number
	selected: Set<string>
	customText: string
	onToggle: (index: number, label: string) => void
	onCustomChange: (index: number, value: string) => void
	onSubmitCustom?: () => void
	disabled: boolean
}

function QuestionSection({
	info,
	index,
	selected,
	customText,
	onToggle,
	onCustomChange,
	onSubmitCustom,
	disabled,
}: QuestionSectionProps) {
	const allowCustom = info.isOther !== false

	return (
		<fieldset aria-label={info.header} className="m-0 border-none p-0">
			<legend className="sr-only">{info.question}</legend>
			<div role="radiogroup" aria-label={info.header} className="flex flex-col px-1">
				{info.options.map((option: { label: string; description: string }) => {
					const isSelected = selected.has(option.label)
					return (
						<button
							key={option.label}
							type="button"
							role="radio"
							aria-checked={isSelected}
							onClick={() => onToggle(index, option.label)}
							disabled={disabled}
							className={cn(
								"flex w-full items-start gap-2 rounded-lg px-2 py-1.5 text-left text-[13px] leading-snug transition-colors",
								isSelected ? "bg-muted text-foreground" : "text-popover-foreground hover:bg-muted/70",
								disabled ? "cursor-not-allowed opacity-45 hover:bg-transparent" : "cursor-pointer",
							)}
						>
							<span
								className={cn(
									"mt-0.5 flex size-3.5 shrink-0 items-center justify-center",
									isSelected ? "text-foreground" : "text-transparent",
								)}
								aria-hidden="true"
							>
								<CheckIcon className="size-3.5 stroke-[1.5]" />
							</span>
							<span className="min-w-0 flex-1">
								<span className="font-normal">{option.label}</span>
								{option.description ? (
									<span className="mt-0.5 block text-[12px] leading-4 text-muted-foreground">
										{option.description}
									</span>
								) : null}
							</span>
						</button>
					)
				})}
			</div>
			{allowCustom ? (
				<div className="px-2 pb-1.5 pt-0.5">
					<label htmlFor={`question-custom-${index}`} className="sr-only">
						Other answer for {info.header || info.question}
					</label>
					<input
						id={`question-custom-${index}`}
						type={info.isSecret ? "password" : "text"}
						value={customText}
						onChange={(e) => onCustomChange(index, e.target.value)}
						onKeyDown={(e) => {
							if (e.key === "Enter" && !e.shiftKey) {
								e.preventDefault()
								onSubmitCustom?.()
							}
						}}
						placeholder={info.isSecret ? "Type a secret value…" : "Or type your own answer…"}
						disabled={disabled}
						className="h-8 w-full rounded-lg border border-border/60 bg-transparent px-2 text-[13px] text-foreground placeholder:text-muted-foreground/60 outline-none transition-colors focus:border-border focus:bg-muted/40 disabled:cursor-not-allowed disabled:opacity-50"
					/>
				</div>
			) : null}
		</fieldset>
	)
}

function AnswerReview({
	questions,
	selections,
	customTexts,
	onSelectQuestion,
	disabled,
}: {
	questions: QuestionInfo[]
	selections: Map<number, Set<string>>
	customTexts: Map<number, string>
	onSelectQuestion: (index: number) => void
	disabled: boolean
}) {
	return (
		<ul className="flex flex-col px-1">
			{questions.map((info, index) => {
				const answer = answerLabel(info, index, selections, customTexts)
				return (
					<li key={info.id || `question-${index}`}>
						<button
							type="button"
							onClick={() => onSelectQuestion(index)}
							disabled={disabled}
							className={cn(
								"flex w-full flex-col items-start gap-0.5 rounded-lg px-2 py-1.5 text-left transition-colors hover:bg-muted/70",
								disabled && "cursor-not-allowed opacity-45 hover:bg-transparent",
							)}
						>
							<span className="text-[13px] leading-5 text-muted-foreground">
								{info.header || info.question}
							</span>
							<span
								className={cn(
									"text-[13px] leading-5",
									answer ? "text-foreground" : "text-muted-foreground/70",
								)}
							>
								{answer ?? "No answer yet"}
							</span>
						</button>
					</li>
				)
			})}
		</ul>
	)
}

function StepDots({
	total,
	current,
	answered,
}: {
	total: number
	current: number
	answered: Set<number>
}) {
	if (total <= 1) return null
	const dots = []
	for (let i = 0; i < total; i++) {
		dots.push(
			<span
				key={`dot-${i}-of-${total}`}
				className={cn(
					"size-1.5 rounded-full transition-colors",
					i === current
						? "bg-foreground"
						: answered.has(i)
							? "bg-foreground/40"
							: "bg-muted-foreground/25",
				)}
				aria-hidden="true"
			/>,
		)
	}
	return (
		<span className="flex items-center gap-1" aria-hidden="true">
			{dots}
		</span>
	)
}

function QuestionNavButton({
	label,
	disabled,
	onClick,
	children,
}: {
	label: string
	disabled: boolean
	onClick: () => void
	children: ReactNode
}) {
	return (
		<button
			type="button"
			aria-label={label}
			title={label}
			disabled={disabled}
			onClick={onClick}
			className="grid size-7 shrink-0 place-items-center rounded-full text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:pointer-events-none disabled:opacity-40"
		>
			{children}
		</button>
	)
}

export const ChatQuestionFlow = memo(function ChatQuestionFlow({
	questions,
	onReply,
	onReject,
	disabled = false,
	isFromSubAgent = false,
}: ChatQuestionFlowProps) {
	const currentRequest = questions[0]
	if (!currentRequest) return null

	return (
		<QuestionRequestStepper
			key={currentRequest.id}
			request={currentRequest}
			totalRequests={questions.length}
			onReply={onReply}
			onReject={onReject}
			disabled={disabled}
			isFromSubAgent={isFromSubAgent}
		/>
	)
})

interface QuestionRequestStepperProps {
	request: QuestionRequest
	totalRequests: number
	onReply: (requestId: string, answers: QuestionAnswer[]) => Promise<void>
	onReject: (requestId: string) => Promise<void>
	disabled: boolean
	isFromSubAgent?: boolean
}

const QuestionRequestStepper = memo(function QuestionRequestStepper({
	request,
	totalRequests,
	onReply,
	onReject,
	disabled,
	isFromSubAgent = false,
}: QuestionRequestStepperProps) {
	const questions = request.questions
	const questionCount = questions.length
	const reviewStep = questionCount
	const pageCount = questionCount + 1

	const [currentStep, setCurrentStep] = useState(0)
	const [selections, setSelections] = useState<Map<number, Set<string>>>(() => new Map())
	const [customTexts, setCustomTexts] = useState<Map<number, string>>(() => new Map())
	const [submitting, setSubmitting] = useState(false)
	const cardRef = useRef<HTMLElement>(null)

	const isReview = currentStep === reviewStep
	const currentQuestion = isReview ? undefined : questions[currentStep]
	const currentAnswered = !isReview && isQuestionAnswered(currentStep, selections, customTexts)
	const allAnswered = questions.every((_, index) =>
		isQuestionAnswered(index, selections, customTexts),
	)

	const answeredSteps = new Set<number>()
	for (let i = 0; i < questionCount; i++) {
		if (isQuestionAnswered(i, selections, customTexts)) {
			answeredSteps.add(i)
		}
	}
	if (allAnswered) answeredSteps.add(reviewStep)

	const handleToggle = useCallback((questionIndex: number, label: string) => {
		setCustomTexts((prev) => {
			if (!prev.has(questionIndex)) return prev
			const next = new Map(prev)
			next.delete(questionIndex)
			return next
		})
		setSelections((prev) => {
			const next = new Map(prev)
			const current = new Set<string>()
			current.add(label)
			next.set(questionIndex, current)
			return next
		})
	}, [])

	const handleCustomChange = useCallback((questionIndex: number, value: string) => {
		if (value.trim()) {
			setSelections((prev) => {
				if (!prev.has(questionIndex)) return prev
				const next = new Map(prev)
				next.delete(questionIndex)
				return next
			})
		}
		setCustomTexts((prev) => {
			const next = new Map(prev)
			next.set(questionIndex, value)
			return next
		})
	}, [])

	const handlePrevPage = useCallback(() => {
		if (disabled || submitting) return
		setCurrentStep((step) => Math.max(0, step - 1))
	}, [disabled, submitting])

	const handleNextPage = useCallback(() => {
		if (disabled || submitting) return
		setCurrentStep((step) => Math.min(pageCount - 1, step + 1))
	}, [disabled, submitting, pageCount])

	const handleNext = useCallback(() => {
		if (isReview || !currentAnswered || disabled || submitting) return
		setCurrentStep((step) => Math.min(pageCount - 1, step + 1))
	}, [isReview, currentAnswered, disabled, submitting, pageCount])

	const handleSubmit = useCallback(async () => {
		if (disabled || submitting || !allAnswered) return
		setSubmitting(true)
		try {
			const answers = buildAnswers(questions, selections, customTexts)
			await onReply(request.id, answers)
		} finally {
			setSubmitting(false)
		}
	}, [disabled, submitting, allAnswered, questions, selections, customTexts, onReply, request.id])

	const handleAdvance = useCallback(() => {
		if (isReview) {
			handleSubmit()
		} else {
			handleNext()
		}
	}, [isReview, handleSubmit, handleNext])

	const handleSkip = useCallback(async () => {
		if (disabled || submitting) return
		setSubmitting(true)
		try {
			await onReject(request.id)
		} finally {
			setSubmitting(false)
		}
	}, [disabled, submitting, onReject, request.id])

	useEffect(() => {
		function handleKeyDown(e: KeyboardEvent) {
			if (e.target instanceof HTMLInputElement && e.target.id?.startsWith("question-custom-")) {
				return
			}
			if (e.key === "Enter" && !e.shiftKey) {
				if (isReview ? allAnswered : currentAnswered) {
					e.preventDefault()
					handleAdvance()
				}
			} else if (e.key === "Escape") {
				e.preventDefault()
				handleSkip()
			} else if (e.key === "ArrowLeft") {
				e.preventDefault()
				handlePrevPage()
			} else if (e.key === "ArrowRight") {
				e.preventDefault()
				handleNextPage()
			}
		}
		document.addEventListener("keydown", handleKeyDown)
		return () => document.removeEventListener("keydown", handleKeyDown)
	}, [
		isReview,
		allAnswered,
		currentAnswered,
		handleAdvance,
		handleSkip,
		handlePrevPage,
		handleNextPage,
	])

	useEffect(() => {
		const timer = requestAnimationFrame(() => {
			if (isReview) {
				cardRef.current?.focus()
				return
			}
			const customInput = document.getElementById(
				`question-custom-${currentStep}`,
			) as HTMLInputElement | null
			if (customInput) {
				customInput.focus()
			} else {
				cardRef.current?.focus()
			}
		})
		return () => cancelAnimationFrame(timer)
	}, [currentStep, isReview])

	const headerTitle = isReview ? "Your answers" : currentQuestion?.header
	const headerBody = isReview
		? "Review before sending. Select an answer to edit it."
		: currentQuestion?.question

	return (
		<section
			ref={cardRef}
			tabIndex={-1}
			aria-label="Agent question"
			className="devo-composer animate-in fade-in slide-in-from-bottom-2 bg-background/95 shadow-[0_8px_32px_rgba(0,0,0,0.05)] outline-none duration-200 dark:shadow-[0_10px_36px_rgba(0,0,0,0.28)]"
		>
			<div className="px-3 pt-3">
				{isFromSubAgent ? (
					<p className="mb-1.5 text-[11px] font-medium text-muted-foreground">From a sub-agent</p>
				) : null}
				<div className="flex items-start gap-2">
					<div className="min-w-0 flex-1">
						{headerTitle ? (
							<div className="text-[13px] font-medium text-muted-foreground">{headerTitle}</div>
						) : null}
						{headerBody ? (
							<div className={cn("text-[13px] leading-5 text-foreground", headerTitle && "mt-0.5")}>
								{headerBody}
							</div>
						) : null}
					</div>
					<div className="flex shrink-0 items-center gap-0.5">
						{totalRequests > 1 ? (
							<span className="mr-1 text-[11px] text-muted-foreground">+{totalRequests - 1} more</span>
						) : null}
						<QuestionNavButton
							label="Previous question"
							disabled={currentStep <= 0 || disabled || submitting}
							onClick={handlePrevPage}
						>
							<ChevronLeftIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
						</QuestionNavButton>
						<QuestionNavButton
							label="Next question"
							disabled={currentStep >= pageCount - 1 || disabled || submitting}
							onClick={handleNextPage}
						>
							<ChevronRightIcon className="size-3.5 stroke-[1.5]" aria-hidden="true" />
						</QuestionNavButton>
					</div>
				</div>
			</div>

			<div className="pt-1.5">
				{isReview ? (
					<AnswerReview
						questions={questions}
						selections={selections}
						customTexts={customTexts}
						onSelectQuestion={setCurrentStep}
						disabled={disabled || submitting}
					/>
				) : currentQuestion ? (
					<QuestionSection
						info={currentQuestion}
						index={currentStep}
						selected={selections.get(currentStep) ?? new Set()}
						customText={customTexts.get(currentStep) ?? ""}
						onToggle={handleToggle}
						onCustomChange={handleCustomChange}
						onSubmitCustom={currentAnswered ? handleAdvance : undefined}
						disabled={disabled || submitting}
					/>
				) : null}
			</div>

			<div className="flex items-center gap-1 px-2 pb-2 pt-1">
				<div className="flex min-w-0 flex-1 items-center gap-2 px-1">
					<StepDots total={pageCount} current={currentStep} answered={answeredSteps} />
				</div>
				<button
					type="button"
					onClick={handleSkip}
					disabled={disabled || submitting}
					className="h-8 rounded-md px-2 text-[13px] text-muted-foreground transition-colors hover:bg-muted hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"
					aria-label="Skip question"
				>
					Skip
				</button>
				{isReview ? (
					<Button
						size="icon-sm"
						onClick={handleSubmit}
						disabled={!allAnswered || disabled || submitting}
						className="size-8 rounded-full"
						aria-label="Send answers"
					>
						{submitting ? (
							<Loader2Icon className="size-4 animate-spin stroke-[1.5]" aria-hidden="true" />
						) : (
							<ArrowUpIcon className="size-4" aria-hidden="true" />
						)}
					</Button>
				) : (
					<Button
						size="icon-sm"
						onClick={handleNext}
						disabled={!currentAnswered || disabled || submitting}
						className="size-8 rounded-full"
						aria-label="Next question"
					>
						<ArrowRightIcon className="size-4" aria-hidden="true" />
					</Button>
				)}
			</div>
		</section>
	)
})
