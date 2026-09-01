import {
	Message,
	MessageAction,
	MessageActions,
	MessageContent,
} from "@devo/ui/components/ai-elements/message"
import { Button } from "@devo/ui/components/button"
import { cn } from "@devo/ui/lib/utils"
import { CheckIcon, CopyIcon, PencilIcon } from "lucide-react"
import { useCallback, useEffect, useRef, useState, type KeyboardEvent, type ReactNode } from "react"

interface UserMessageBlockProps {
	text: string
	canEdit?: boolean
	onEdit?: (text: string) => Promise<void>
	children?: ReactNode
}

export function UserMessageBlock({
	text,
	canEdit = false,
	onEdit,
	children,
}: UserMessageBlockProps) {
	const [editing, setEditing] = useState(false)
	const [draft, setDraft] = useState(text)
	const [copied, setCopied] = useState(false)
	const [saving, setSaving] = useState(false)
	const textareaRef = useRef<HTMLTextAreaElement>(null)
	const showCopy = text.length > 0
	const showActions = !editing && (showCopy || canEdit)
	const canSubmit = draft.trim().length > 0 && !saving && !!onEdit

	useEffect(() => {
		if (!editing) setDraft(text)
	}, [editing, text])

	useEffect(() => {
		if (!canEdit && editing) {
			setEditing(false)
			setDraft(text)
		}
	}, [canEdit, editing, text])

	useEffect(() => {
		if (!editing) return
		const textarea = textareaRef.current
		if (!textarea) return
		textarea.focus()
		textarea.selectionStart = textarea.value.length
		textarea.selectionEnd = textarea.value.length
	}, [editing])

	const handleCopy = useCallback(async () => {
		if (!text) return
		await navigator.clipboard.writeText(text)
		setCopied(true)
		setTimeout(() => setCopied(false), 2000)
	}, [text])

	const handleStartEdit = useCallback(() => {
		setDraft(text)
		setEditing(true)
	}, [text])

	const handleCancel = useCallback(() => {
		if (saving) return
		setEditing(false)
		setDraft(text)
	}, [saving, text])

	const handleSave = useCallback(async () => {
		const next = draft.trim()
		if (!next || saving || !onEdit) return
		setSaving(true)
		try {
			await onEdit(next)
			setEditing(false)
		} finally {
			setSaving(false)
		}
	}, [draft, onEdit, saving])

	const handleKeyDown = useCallback(
		(event: KeyboardEvent<HTMLTextAreaElement>) => {
			if (event.key === "Escape") {
				event.preventDefault()
				handleCancel()
				return
			}
			if (event.key !== "Enter" || event.shiftKey || event.nativeEvent.isComposing) return
			event.preventDefault()
			void handleSave()
		},
		[handleCancel, handleSave],
	)

	return (
		<Message from="user">
			<span className="relative inline-block max-w-[min(36rem,85%)] text-left align-top">
			<MessageContent>
				{children}
				{editing ? (
					<textarea
						ref={textareaRef}
						value={draft}
						onChange={(event) => setDraft(event.target.value)}
						onKeyDown={handleKeyDown}
						disabled={saving}
						rows={Math.min(12, Math.max(2, draft.split("\n").length))}
						className="field-sizing-content w-full min-w-48 resize-none bg-transparent p-0 text-[14px] leading-[1.55] tracking-[-0.01em] text-foreground outline-none"
					/>
				) : (
					<p className="whitespace-pre-wrap [overflow-wrap:break-word] [word-break:normal]">
						{text}
					</p>
				)}
			</MessageContent>
			{editing && (
				<div className="mt-1 flex items-center justify-end gap-1.5">
					<Button type="button" size="xs" variant="ghost" onClick={handleCancel} disabled={saving}>
						Cancel
					</Button>
					<Button type="button" size="xs" onClick={() => void handleSave()} disabled={!canSubmit}>
						{saving ? "Sending..." : "Send"}
					</Button>
				</div>
			)}
			{showActions && (
				<MessageActions
					className={cn(
						"absolute top-full right-0 z-10 pt-0.5",
						"opacity-100 [@media(hover:hover)]:pointer-events-none [@media(hover:hover)]:opacity-0",
						"[@media(hover:hover)]:transition-opacity [@media(hover:hover)]:duration-150",
						"[@media(hover:hover)]:group-hover:pointer-events-auto [@media(hover:hover)]:group-hover:opacity-100",
						"[@media(hover:hover)]:group-focus-within:pointer-events-auto [@media(hover:hover)]:group-focus-within:opacity-100",
					)}
				>
					{showCopy && (
						<MessageAction
							tooltip={copied ? "Copied" : "Copy message"}
							onClick={handleCopy}
						>
							{copied ? <CheckIcon className="size-3" /> : <CopyIcon className="size-3" />}
						</MessageAction>
					)}
					{canEdit && (
						<MessageAction tooltip="Edit message" onClick={handleStartEdit}>
							<PencilIcon className="size-3" />
						</MessageAction>
					)}
				</MessageActions>
			)}
			</span>
		</Message>
	)
}
