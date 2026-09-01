import {
	PromptInput,
	PromptInputButton,
	PromptInputFooter,
	PromptInputProvider,
	PromptInputSubmit,
	PromptInputTextarea,
	PromptInputTools,
	usePromptInputAttachments,
	usePromptInputController,
} from "@devo/ui/components/ai-elements/prompt-input"
import { PlusIcon } from "lucide-react"
import { useCallback, useEffect, useRef, useState, useTransition } from "react"
import { setProjectModelAtom } from "../../atoms/preferences"
import { appStore } from "../../atoms/store"
import { useDraftActions, useDraftSnapshot } from "../../hooks/use-draft"
import type { ConfigData, ModelRef, ProvidersData, SdkAgent } from "../../hooks/use-devo-data"
import {
	getModelInputCapabilities,
	resolveEffectiveModel,
	useModelState,
} from "../../hooks/use-devo-data"

import type { Agent, FileAttachment } from "../../lib/types"
import { ContextItems } from "./context-items"
import { type MentionOption, MentionPopover, type MentionPopoverHandle } from "./mention-popover"
import { PromptAttachmentPreview } from "./prompt-attachments"
import {
	createMentionFromOption,
	getMentionKey,
	getMentionMarker,
	insertMentionIntoText,
	type PromptMention,
	reconcileMentions,
} from "./prompt-mentions"
import { PromptToolbar } from "./prompt-toolbar"
import { SlashCommandPopover, type SlashCommandPopoverHandle } from "./slash-command-popover"

interface ChatInputProps {
	agent: Agent
	isConnected: boolean
	onSendMessage?: (
		agent: Agent,
		message: string,
		options?: { model?: ModelRef; agentName?: string; variant?: string; files?: FileAttachment[] },
	) => Promise<void>
	onStop?: (agent: Agent) => Promise<void>
	providers?: ProvidersData | null
	config?: ConfigData | null
	devoAgents?: SdkAgent[]
	onScrollToBottom: (behavior?: "instant" | "smooth") => void
	handleSlashCommand: (text: string) => Promise<boolean>
}

function AttachButton({ disabled }: { disabled?: boolean }) {
	const attachments = usePromptInputAttachments()
	return (
		<PromptInputButton
			tooltip="Attach files"
			onClick={() => attachments.openFileDialog()}
			disabled={disabled}
			className="size-8 rounded-full bg-muted/80 text-muted-foreground hover:bg-muted hover:text-foreground"
		>
			<PlusIcon className="size-4" />
		</PromptInputButton>
	)
}

function DraftSync({ setDraft }: { setDraft: (text: string) => void }) {
	const controller = usePromptInputController()
	const value = controller.textInput.value
	const isFirstRender = useRef(true)

	useEffect(() => {
		if (isFirstRender.current) {
			isFirstRender.current = false
			return
		}
		setDraft(value)
	}, [value, setDraft])

	return null
}

function SlashCommandBridge({
	controllerRef,
}: {
	controllerRef: React.RefObject<{ setText: (text: string) => void; getText: () => string } | null>
}) {
	const controller = usePromptInputController()
	useEffect(() => {
		if (controllerRef && "current" in controllerRef) {
			;(controllerRef as React.MutableRefObject<typeof controllerRef.current>).current = {
				setText: (text: string) => controller.textInput.setInput(text),
				getText: () => controller.textInput.value,
			}
		}
		return () => {
			if (controllerRef && "current" in controllerRef) {
				;(controllerRef as React.MutableRefObject<typeof controllerRef.current>).current = null
			}
		}
	}, [controller, controllerRef])
	return null
}

function TriggerDetector({
	onSlashChange,
	onMentionChange,
}: {
	onSlashChange: (open: boolean, query: string) => void
	onMentionChange: (open: boolean, query: string) => void
}) {
	const controller = usePromptInputController()
	const inputText = controller.textInput.value
	useEffect(() => {
		const textarea = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
		const cursorPos = textarea?.selectionStart ?? inputText.length
		const textBeforeCursor = inputText.slice(0, cursorPos)
		const slashMatch = inputText.match(/^\/(\S*)$/)
		if (slashMatch) {
			onSlashChange(true, slashMatch[1])
			onMentionChange(false, "")
			return
		}
		const atMatch = textBeforeCursor.match(/@(\S*)$/)
		if (atMatch) {
			onMentionChange(true, atMatch[1])
			onSlashChange(false, "")
			return
		}
		onSlashChange(false, "")
		onMentionChange(false, "")
	}, [inputText, onSlashChange, onMentionChange])
	return null
}

function MentionReconciler({
	mentions,
	onReconcile,
}: {
	mentions: PromptMention[]
	onReconcile: (updated: PromptMention[]) => void
}) {
	const controller = usePromptInputController()
	const inputText = controller.textInput.value
	useEffect(() => {
		if (mentions.length === 0) return
		const reconciled = reconcileMentions(mentions, inputText)
		if (reconciled.length !== mentions.length) {
			onReconcile(reconciled)
		}
	}, [inputText, mentions, onReconcile])
	return null
}

export function ChatInput({
	agent,
	isConnected,
	onSendMessage,
	onStop,
	providers,
	config,
	devoAgents,
	onScrollToBottom,
	handleSlashCommand,
}: ChatInputProps) {
	const isWorking = agent.status === "running"
	const [sending, setSending] = useState(false)
	const [mentions, setMentions] = useState<PromptMention[]>([])
	const [, startTransition] = useTransition()

	const { setDraft, clearDraft } = useDraftActions(agent.sessionId)
	const draft = useDraftSnapshot(agent.sessionId)

	const [selectedModel, setSelectedModel] = useState<ModelRef | null>(null)
	const [selectedAgent, setSelectedAgent] = useState<string | null>(null)
	const [selectedVariant, setSelectedVariant] = useState<string | undefined>(undefined)

	const { addRecent: addRecentModel } = useModelState()

	// Resolve effective model

	const effectiveModel = resolveEffectiveModel(
		selectedModel,
		devoAgents?.find((a) => a.name === (selectedAgent ?? config?.defaultAgent)) ?? null,
		config?.model,
		providers?.defaults ?? {},
		providers?.providers ?? [],
	)

	const modelCapabilities = getModelInputCapabilities(effectiveModel, providers?.providers ?? [])

	// Popover state
	const [slashOpen, setSlashOpen] = useState(false)
	const [slashQuery, setSlashQuery] = useState("")
	const [mentionOpen, setMentionOpen] = useState(false)
	const [mentionQuery, setMentionQuery] = useState("")

	const slashPopoverRef = useRef<SlashCommandPopoverHandle>(null)
	const mentionPopoverRef = useRef<MentionPopoverHandle>(null)
	const slashCommandRef = useRef<{ setText: (t: string) => void; getText: () => string } | null>(
		null,
	)

	const handleSend = useCallback(
		async (text: string, files?: FileAttachment[]) => {
			if (!text.trim() || !onSendMessage || sending) return

			if (text.trim().startsWith("/")) {
				const handled = await handleSlashCommand(text)
				if (handled) {
					slashCommandRef.current?.setText("")
					clearDraft()
					setMentions([])
					return
				}
			}

			setSending(true)
			try {
				if (effectiveModel && agent.directory) {
					appStore.set(setProjectModelAtom, {
						directory: agent.directory,
						model: {
							...effectiveModel,
							variant: selectedVariant,
							agent: selectedAgent || undefined,
						},
					})
				}
				await onSendMessage(agent, text.trim(), {
					model: effectiveModel ?? undefined,
					agentName: selectedAgent || undefined,
					variant: selectedVariant,
					files,
				})
				clearDraft()
				setMentions([])
				onScrollToBottom("smooth")
			} finally {
				setSending(false)
			}
		},
		[
			onSendMessage,
			sending,
			agent,
			effectiveModel,
			selectedAgent,
			selectedVariant,
			clearDraft,
			onScrollToBottom,
			handleSlashCommand,
		],
	)

	const handleMentionSelect = useCallback((option: MentionOption) => {
		setMentionOpen(false)
		const ctrl = slashCommandRef.current
		if (!ctrl) return
		const currentText = ctrl.getText()
		const textarea = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
		const cursorPos = textarea?.selectionStart ?? currentText.length
		const mention = createMentionFromOption(option)
		const { text: newText, cursorPosition: newCursor } = insertMentionIntoText(
			currentText,
			cursorPos,
			mention,
		)
		ctrl.setText(newText)
		setMentions((prev) => {
			const key = getMentionKey(mention)
			if (prev.some((candidate) => getMentionKey(candidate) === key))
				return prev
			return [...prev, mention]
		})
		requestAnimationFrame(() => {
			const ta = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
			if (ta) {
				ta.focus()
				ta.setSelectionRange(newCursor, newCursor)
			}
		})
	}, [])

	const handleTextareaKeyDown = useCallback(
		(e: React.KeyboardEvent<HTMLTextAreaElement>) => {
			// Always delegate to popovers first — they guard on their own `open` prop
			// internally, avoiding stale-closure issues with slashOpen/mentionOpen.
			if (slashPopoverRef.current?.handleKeyDown(e)) return
			if (mentionPopoverRef.current?.handleKeyDown(e)) return
		},
		[],
	)

	return (
		<PromptInputProvider key={agent.sessionId} initialInput={draft}>
			<DraftSync setDraft={setDraft} />
			<SlashCommandBridge controllerRef={slashCommandRef} />
			<TriggerDetector
				onSlashChange={(open, query) => {
					setSlashOpen(open)
					setSlashQuery(query)
				}}
				onMentionChange={(open, query) => {
					setMentionOpen(open)
					setMentionQuery(query)
				}}
			/>
			<MentionReconciler mentions={mentions} onReconcile={setMentions} />
			<div className="relative">
				<SlashCommandPopover
					ref={slashPopoverRef}
					query={slashQuery}
					open={slashOpen}
					enabled={isConnected}
					onSelect={(cmd) => {
						setSlashOpen(false)
						// Use the command string directly instead of setText + setTimeout
						// round-trip, which races with React's async state batching.
						if (cmd.startsWith("/")) {
							handleSlashCommand(cmd).then((handled) => {
								if (handled) {
									slashCommandRef.current?.setText("")
									clearDraft()
									setMentions([])
								} else {
									// Not recognized — leave it in input for normal send
									slashCommandRef.current?.setText(cmd)
								}
							})
						} else {
							slashCommandRef.current?.setText(cmd)
						}
					}}
					onClose={() => setSlashOpen(false)}
				/>
				<MentionPopover
					ref={mentionPopoverRef}
					query={mentionQuery}
					open={mentionOpen}
					directory={agent.directory}
					agents={devoAgents ?? []}
					onSelect={handleMentionSelect}
					onClose={() => setMentionOpen(false)}
				/>
				<PromptInput
					className="devo-composer"
					onSubmit={(message) => {
						if (message.text.trim() && isConnected && !sending)
							handleSend(message.text, message.files.length > 0 ? message.files : undefined)
					}}
				>
					<ContextItems
						mentions={mentions}
						onRemove={(m) => {
							const marker = getMentionMarker(m)
							const ctrl = slashCommandRef.current
							if (ctrl) {
								const currentText = ctrl.getText()
								ctrl.setText(currentText.replace(`${marker} `, "").replace(marker, ""))
							}
							setMentions((prev) => prev.filter((x) => x !== m))
						}}
					/>
					<PromptAttachmentPreview
						supportsImages={modelCapabilities?.image}
						supportsPdf={modelCapabilities?.pdf}
					/>
					<PromptInputTextarea
						data-prompt-input
						onKeyDown={handleTextareaKeyDown}
						placeholder={isWorking ? "Send a follow-up message..." : "What would you like to do?"}
						disabled={!isConnected}
					/>
					<PromptInputFooter>
						<PromptInputTools>
							<AttachButton disabled={!isConnected} />
						</PromptInputTools>
						<div className="ml-auto flex min-w-0 items-center gap-0.5">
							<PromptToolbar
								agents={devoAgents ?? []}
								selectedAgent={selectedAgent}
								defaultAgent={config?.defaultAgent}
								onSelectAgent={(a) => startTransition(() => setSelectedAgent(a))}
								providers={providers ?? null}
								effectiveModel={effectiveModel}
								hasModelOverride={!!selectedModel}
								onSelectModel={(m) =>
									startTransition(() => {
										setSelectedModel(m)
										setSelectedVariant(undefined)
										if (m) addRecentModel(m)
									})
								}
								selectedVariant={selectedVariant}
								onSelectVariant={(v) => startTransition(() => setSelectedVariant(v))}
								disabled={!isConnected}
							/>
							<PromptInputSubmit
								disabled={!isConnected || sending}
								status={isWorking ? "streaming" : undefined}
								onStop={() => onStop?.(agent)}
							/>
						</div>
					</PromptInputFooter>
				</PromptInput>
			</div>
		</PromptInputProvider>
	)
}
