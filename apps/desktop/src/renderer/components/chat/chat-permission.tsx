import { Button } from "@devo/ui/components/button"
import { cn } from "@devo/ui/lib/utils"
import { ArrowUpIcon, CheckIcon, Loader2Icon } from "lucide-react"
import { memo, useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react"
import type { Agent, PermissionRequest, PermissionResponse } from "../../lib/types"
import {
	buildApprovalChoices,
	permissionSummaryLines,
	type ApprovalChoice,
} from "./chat-permission-options"

interface ChatPermissionFlowProps {
	agent: Agent
	permission: PermissionRequest
	onApprove?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		response?: PermissionResponse,
	) => Promise<void>
	onDeny?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		note?: string,
	) => Promise<void>
	disabled?: boolean
	isFromSubAgent?: boolean
}

export const ChatPermissionFlow = memo(function ChatPermissionFlow({
	agent,
	permission,
	onApprove,
	onDeny,
	disabled = false,
	isFromSubAgent = false,
}: ChatPermissionFlowProps) {
	const choices = useMemo(() => buildApprovalChoices(permission), [permission])
	const summary = useMemo(() => permissionSummaryLines(permission), [permission])
	const answerable = permission.metadata?.answerable !== false
	const [selectedIndex, setSelectedIndex] = useState(0)
	const [denyNote, setDenyNote] = useState("")
	const [responding, setResponding] = useState(false)
	const cardRef = useRef<HTMLElement>(null)
	const denyInputRef = useRef<HTMLInputElement>(null)

	const selectedChoice = choices[selectedIndex] ?? choices[0]
	const isDenySelected = selectedChoice?.kind === "deny"

	useEffect(() => {
		setSelectedIndex(0)
		setDenyNote("")
	}, [permission.id])

	useEffect(() => {
		if (!isDenySelected) return
		const timer = requestAnimationFrame(() => denyInputRef.current?.focus())
		return () => cancelAnimationFrame(timer)
	}, [isDenySelected, selectedIndex])

	useEffect(() => {
		if (isDenySelected) return
		denyInputRef.current?.blur()
	}, [isDenySelected])

	const handleSubmit = useCallback(async () => {
		if (!selectedChoice || responding || disabled || !answerable) return
		setResponding(true)
		try {
			if (selectedChoice.kind === "deny") {
				await onDeny?.(agent, permission.sessionID, permission.id, denyNote.trim() || undefined)
			} else {
				await onApprove?.(agent, permission.sessionID, permission.id, selectedChoice.scope)
			}
		} finally {
			setResponding(false)
		}
	}, [
		agent,
		answerable,
		denyNote,
		disabled,
		onApprove,
		onDeny,
		permission.id,
		permission.sessionID,
		responding,
		selectedChoice,
	])

	const moveSelection = useCallback(
		(delta: number) => {
			setSelectedIndex((index) => {
				if (choices.length === 0) return 0
				return (index + delta + choices.length) % choices.length
			})
		},
		[choices.length],
	)

	useEffect(() => {
		function handleKeyDown(event: KeyboardEvent) {
			if (event.target instanceof HTMLInputElement && event.target.id === "permission-deny-note") {
				if (event.key === "ArrowDown") {
					event.preventDefault()
					moveSelection(1)
					return
				}
				if (event.key === "ArrowUp") {
					event.preventDefault()
					moveSelection(-1)
					return
				}
				if (event.key === "Enter" && !event.shiftKey) {
					event.preventDefault()
					void handleSubmit()
				}
				return
			}

			if (event.key === "ArrowDown") {
				event.preventDefault()
				moveSelection(1)
			} else if (event.key === "ArrowUp") {
				event.preventDefault()
				moveSelection(-1)
			} else if (event.key === "Enter" && !event.shiftKey) {
				event.preventDefault()
				void handleSubmit()
			} else if (event.key === "Escape") {
				event.preventDefault()
				setSelectedIndex(choices.findIndex((choice) => choice.kind === "deny"))
			}
		}
		document.addEventListener("keydown", handleKeyDown)
		return () => document.removeEventListener("keydown", handleKeyDown)
	}, [choices, handleSubmit, moveSelection])

	useEffect(() => {
		const timer = requestAnimationFrame(() => cardRef.current?.focus())
		return () => cancelAnimationFrame(timer)
	}, [permission.id])

	return (
		<section
			ref={cardRef}
			tabIndex={-1}
			aria-label="Tool permission request"
			className="devo-composer animate-in fade-in slide-in-from-bottom-2 bg-background/95 shadow-[0_8px_32px_rgba(0,0,0,0.05)] outline-none duration-200 dark:shadow-[0_10px_36px_rgba(0,0,0,0.28)]"
		>
			<div className="px-3 pt-3">
				{isFromSubAgent ? (
					<p className="mb-1.5 text-[11px] font-medium text-muted-foreground">From a sub-agent</p>
				) : null}
				<div className="text-[13px] font-medium text-muted-foreground">Permission required</div>
				<div className="mt-0.5 text-[13px] leading-5 text-foreground">{summary.title}</div>
				{!answerable ? (
					<p className="mt-2 flex items-center gap-1.5 text-[12px] text-muted-foreground">
						<Loader2Icon className="size-3 animate-spin" />
						Restoring connection…
					</p>
				) : null}
				{summary.reason ? (
					<p className="mt-1 text-[12px] leading-4 text-muted-foreground">{summary.reason}</p>
				) : null}
			</div>

			<div className="pt-1.5">
				<fieldset aria-label="Approval options" className="m-0 border-none p-0">
					<div role="listbox" aria-label="Approval options" className="flex flex-col px-1">
						{choices.map((choice, index) => (
							<ChoiceRow
								key={choice.id}
								choice={choice}
								selected={index === selectedIndex}
								disabled={disabled || responding || !answerable}
								denyNote={denyNote}
								denyInputRef={denyInputRef}
								onDenyNoteChange={setDenyNote}
								onSelect={() => setSelectedIndex(index)}
							/>
						))}
					</div>
				</fieldset>
			</div>

			<div className="flex items-center gap-1 px-2 pb-2 pt-1">
				<div className="min-w-0 flex-1 px-1 text-[12px] text-muted-foreground">
					↑↓ to choose · Enter to confirm
				</div>
				<Button
					size="icon-sm"
					onClick={() => void handleSubmit()}
					disabled={disabled || responding || !selectedChoice || !answerable}
					className="size-8 rounded-full"
					aria-label={isDenySelected ? "Confirm denial" : "Confirm approval"}
				>
					{responding ? (
						<Loader2Icon className="size-4 animate-spin stroke-[1.5]" aria-hidden="true" />
					) : (
						<ArrowUpIcon className="size-4" aria-hidden="true" />
					)}
				</Button>
			</div>
		</section>
	)
})

function ChoiceRow({
	choice,
	selected,
	disabled,
	denyNote,
	denyInputRef,
	onDenyNoteChange,
	onSelect,
}: {
	choice: ApprovalChoice
	selected: boolean
	disabled: boolean
	denyNote: string
	denyInputRef: RefObject<HTMLInputElement | null>
	onDenyNoteChange: (value: string) => void
	onSelect: () => void
}) {
	if (choice.kind === "deny" && selected) {
		return (
			<div
				role="option"
				aria-selected
				className={cn(
					"flex w-full items-center gap-2 rounded-lg px-2 py-1.5 text-left text-[13px] leading-snug",
					"bg-muted text-foreground",
				)}
			>
				<span className="flex size-3.5 shrink-0 items-center justify-center" aria-hidden="true">
					<CheckIcon className="size-3.5 stroke-[1.5]" />
				</span>
				<input
					ref={denyInputRef}
					id="permission-deny-note"
					type="text"
					value={denyNote}
					onChange={(event) => onDenyNoteChange(event.target.value)}
					disabled={disabled}
					placeholder="Deny — optional note for the agent"
					className="min-w-0 flex-1 border-none bg-transparent text-[13px] text-foreground outline-none placeholder:text-muted-foreground/70"
				/>
			</div>
		)
	}

	return (
		<button
			type="button"
			role="option"
			aria-selected={selected}
			onClick={onSelect}
			disabled={disabled}
			className={cn(
				"flex w-full items-start gap-2 rounded-lg px-2 py-1.5 text-left text-[13px] leading-snug transition-colors",
				selected ? "bg-muted text-foreground" : "text-popover-foreground hover:bg-muted/70",
				disabled ? "cursor-not-allowed opacity-45 hover:bg-transparent" : "cursor-pointer",
			)}
		>
			<span
				className={cn(
					"mt-0.5 flex size-3.5 shrink-0 items-center justify-center",
					selected ? "text-foreground" : "text-transparent",
				)}
				aria-hidden="true"
			>
				<CheckIcon className="size-3.5 stroke-[1.5]" />
			</span>
			<span className="min-w-0 flex-1 font-normal">{choice.label}</span>
		</button>
	)
}

/** @deprecated Use ChatPermissionFlow in the composer slot. */
export const PermissionItem = ChatPermissionFlow
