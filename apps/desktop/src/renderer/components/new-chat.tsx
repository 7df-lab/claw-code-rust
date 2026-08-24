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
import { type MentionOption, MentionPopover, type MentionPopoverHandle } from "./chat/mention-popover"
import {
	createMentionFromOption,
	insertMentionIntoText,
} from "./chat/prompt-mentions"
import { SlashCommandPopover, type SlashCommandPopoverHandle } from "./chat/slash-command-popover"
import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { useNavigate, useParams } from "@tanstack/react-router"
import { useAtom, useAtomValue } from "jotai"
import {
	GitForkIcon,
	MonitorIcon,
	PlusIcon,
} from "lucide-react"
import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { projectModelsAtom, lastProjectDirectoryAtom, setProjectModelAtom } from "../atoms/preferences"
import {
	removeSessionAtom,
	setSessionBranchAtom,
	setSessionSetupPhaseAtom,
	setSessionWorktreeAtom,
	upsertSessionAtom,
} from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { useDesktopProjectActions } from "./desktop-project-actions-context"
import { useAgents, useProjectList } from "../hooks/use-agents"
import { newChatDraftKey, useDraftActions, useDraftSnapshot } from "../hooks/use-draft"
import type { ModelRef } from "../hooks/use-devo-data"
import {
	getModelInputCapabilities,
	getModelVariants,
	resolveEffectiveModel,
	useConfig,
	useModelState,
	useDevoAgents,
	useProviders,
	useVcs,
} from "../hooks/use-devo-data"
import { useAgentActions } from "../hooks/use-server"
import { persistRuntimeModelConfigOption, persistRuntimeModelSelection } from "../lib/model-config-options"
import { resolveSelectedProjectDirectory, navigateToNewChat } from "../lib/project-selection"
import type { FileAttachment } from "../lib/types"
import { createWorktree, randomWorktreeName } from "../services/worktree-service"
import { BranchPicker } from "./branch-picker"
import { PromptAttachmentPreview } from "./chat/prompt-attachments"
import { PromptToolbar, StatusBar } from "./chat/prompt-toolbar"
import { NewChatProjectPicker } from "./new-chat-project-picker"

// ============================================================
// Worktree mode toggle
// ============================================================

function WorktreeToggle({
	mode,
	onModeChange,
}: {
	mode: "local" | "worktree"
	onModeChange: (mode: "local" | "worktree") => void
}) {
	return (
		<div className="flex items-center rounded-md border border-border/40">
			<Tooltip>
				<TooltipTrigger
					render={
						<button
							type="button"
							onClick={() => onModeChange("local")}
							className={`flex items-center gap-1 rounded-l-md px-1.5 py-0.5 text-[11px] transition-colors ${
								mode === "local"
									? "bg-muted/80 text-foreground"
									: "text-muted-foreground/60 hover:text-muted-foreground"
							}`}
						/>
					}
				>
					<MonitorIcon className="size-3" />
					<span>Local</span>
				</TooltipTrigger>
				<TooltipContent side="top">Run in your current working directory</TooltipContent>
			</Tooltip>
			<Tooltip>
				<TooltipTrigger
					render={
						<button
							type="button"
							onClick={() => onModeChange("worktree")}
							className={`flex items-center gap-1 rounded-r-md px-1.5 py-0.5 text-[11px] transition-colors ${
								mode === "worktree"
									? "bg-muted/80 text-foreground"
									: "text-muted-foreground/60 hover:text-muted-foreground"
							}`}
						/>
					}
				>
					<GitForkIcon className="size-3" />
					<span>Worktree</span>
				</TooltipTrigger>
				<TooltipContent side="top">
					Run in an isolated git worktree (your working copy stays untouched)
				</TooltipContent>
			</Tooltip>
		</div>
	)
}

// ============================================================
// Prompt trigger support helpers (mirrors the pattern in ChatInput)
// ============================================================

/**
 * Exposes the PromptInputProvider's text controller to outside components
 * via a ref — needed to insert slash command and mention text without going
 * through React state.
 */
function MentionBridge({
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

/**
 * User requirement: the new-session composer must support `/` slash popover
 * behavior in addition to existing `@` mentions.
 */
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

/**
 * Syncs PromptInputProvider text to persisted drafts (debounced).
 * Must be rendered inside a <PromptInputProvider>.
 */
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

function AttachButton({ disabled }: { disabled?: boolean }) {
	const attachments = usePromptInputAttachments()
	return (
		<PromptInputButton
			tooltip="Attach files"
			onClick={() => attachments.openFileDialog()}
			disabled={disabled}
		>
			<PlusIcon className="size-4" />
		</PromptInputButton>
	)
}

export function NewChat() {
	const { projectSlug } = useParams({ strict: false }) as { projectSlug?: string }
	const projects = useProjectList()
	const { createSession, sendPrompt } = useAgentActions()
	const navigate = useNavigate()
	const { startFromScratch, useExistingFolder } = useDesktopProjectActions()
	const [lastProjectDirectory, setLastProjectDirectory] = useAtom(lastProjectDirectoryAtom)

	const [selectedDirectory, setSelectedDirectory] = useState<string>(() => lastProjectDirectory ?? "")
	const [launching, setLaunching] = useState(false)
	const [error, setError] = useState<string | null>(null)
	const [worktreeMode, setWorktreeMode] = useState<"local" | "worktree">("local")
	const manuallySelectedDirectoryRef = useRef<string | null>(null)

	// Draft persistence — survives page reloads.
	// Non-reactive snapshot: the draft is only used for PromptInputProvider's
	// initialInput (consumed once on mount), so reactive tracking is unnecessary.
	const draftKey = newChatDraftKey(selectedDirectory)
	const draft = useDraftSnapshot(draftKey)
	const { setDraft, clearDraft } = useDraftActions(draftKey)
	const [unavailableProjectDirectories, setUnavailableProjectDirectories] = useState<ReadonlySet<string>>(() => new Set())

	// Toolbar state
	const [selectedModel, setSelectedModel] = useState<ModelRef | null>(null)
	const [selectedAgent, setSelectedAgent] = useState<string | null>(null)
	const [selectedVariant, setSelectedVariant] = useState<string | undefined>(undefined)

	// Slash command and mention popover state
	const [slashOpen, setSlashOpen] = useState(false)
	const [slashQuery, setSlashQuery] = useState("")
	const [mentionOpen, setMentionOpen] = useState(false)
	const [mentionQuery, setMentionQuery] = useState("")
	const controllerRef = useRef<{ setText: (text: string) => void; getText: () => string } | null>(
		null,
	)
	const slashPopoverRef = useRef<SlashCommandPopoverHandle>(null)
	const mentionPopoverRef = useRef<MentionPopoverHandle>(null)

	// Project model preferences are a UI fallback for older local state.
	const projectModels = useAtomValue(projectModelsAtom)
	const prevDirectoryRef = useRef<string>("")
	useEffect(() => {
		if (!selectedDirectory || selectedDirectory === prevDirectoryRef.current) return
		prevDirectoryRef.current = selectedDirectory
		const stored = projectModels[selectedDirectory]
		if (stored?.providerID && stored?.modelID) {
			setSelectedModel(stored)
			setSelectedVariant(stored.variant)
		} else {
			setSelectedModel(null)
			setSelectedVariant(undefined)
		}
		// Restore the per-project agent preference (null = use config default)
		setSelectedAgent(stored?.agent ?? null)
	}, [selectedDirectory, projectModels])

	const selectedProject = useMemo(
		() => projects.find((p) => p.directory === selectedDirectory),
		[projects, selectedDirectory],
	)

	const handleSelectProject = useCallback(
		(project: (typeof projects)[number]) => {
			manuallySelectedDirectoryRef.current = project.directory
			setSelectedDirectory(project.directory)
			setLastProjectDirectory(project.directory)
			navigate({
				to: "/project/$projectSlug",
				params: { projectSlug: project.slug },
			})
		},
		[navigate, setLastProjectDirectory],
	)

	const handleUseExistingFolder = useCallback(async () => {
		const folder = await useExistingFolder?.()
		if (!folder) return
		manuallySelectedDirectoryRef.current = folder.directory
		setSelectedDirectory(folder.directory)
	}, [useExistingFolder])

	const { data: providers } = useProviders(selectedDirectory || null)
	const { data: config } = useConfig(selectedDirectory || null)
	const { data: vcs, reload: reloadVcs } = useVcs(selectedDirectory || null)
	const { agents: devoAgents } = useDevoAgents(selectedDirectory || null)
	const { recentModels, addRecent: addRecentModel } = useModelState()

	const handleModelSelect = useCallback(
		(model: ModelRef | null) => {
			setSelectedModel(model)
			setSelectedVariant(undefined)
			if (!model) return
			addRecentModel(model)
			if (!selectedDirectory) return
			void persistRuntimeModelSelection(selectedDirectory, model).catch((err) => {
				console.error("Failed to persist model selection:", err)
				setError("Failed to save model setting")
			})
		},
		[addRecentModel, selectedDirectory],
	)

	const handleVariantSelect = useCallback(
		(variant: string | undefined) => {
			setSelectedVariant(variant)
			if (!variant || !selectedDirectory) return
			void persistRuntimeModelConfigOption(selectedDirectory, "thought_level", variant).catch((err) => {
				console.error("Failed to persist reasoning effort selection:", err)
				setError("Failed to save reasoning effort setting")
			})
		},
		[selectedDirectory],
	)

	// Count active sessions on the selected directory (for branch switch warnings)
	const allAgents = useAgents()
	const activeSessionCount = useMemo(() => {
		if (!selectedDirectory) return 0
		return allAgents.filter(
			(a) =>
				a.directory === selectedDirectory && (a.status === "running" || a.status === "waiting"),
		).length
	}, [allAgents, selectedDirectory])

	// Callback when branch is switched via the BranchPicker — forces VCS reload
	const handleBranchChanged = useCallback(
		(_branch: string) => {
			// VCS hook polls every 30s, but we want immediate UI update.
			// The Native vcs.branch.updated event will also fire eventually.
			reloadVcs()
		},
		[reloadVcs],
	)

	const handleSlashClose = useCallback(() => {
		setSlashOpen(false)
		setSlashQuery("")
	}, [])

	const handleMentionClose = useCallback(() => {
		setMentionOpen(false)
		setMentionQuery("")
	}, [])

	const handleSlashSelect = useCallback(
		(command: string) => {
			handleSlashClose()
			const ctrl = controllerRef.current
			if (!ctrl) return
			ctrl.setText(command)
			requestAnimationFrame(() => {
				const ta = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
				if (ta) {
					ta.focus()
					ta.setSelectionRange(command.length, command.length)
				}
			})
		},
		[handleSlashClose],
	)

	// Insert a selected mention into the prompt textarea
	const handleMentionSelect = useCallback(
		(option: MentionOption) => {
			handleMentionClose()
			const ctrl = controllerRef.current
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
			requestAnimationFrame(() => {
				const ta = document.querySelector<HTMLTextAreaElement>("textarea[data-prompt-input]")
				if (ta) {
					ta.focus()
					ta.setSelectionRange(newCursor, newCursor)
				}
			})
		},
		[handleMentionClose],
	)

	// Delegate keyboard events to open popovers before the composer handles Enter.
	const handleTextareaKeyDown = useCallback(
		(e: React.KeyboardEvent<HTMLTextAreaElement>) => {
			if (slashPopoverRef.current?.handleKeyDown(e)) return
			if (mentionPopoverRef.current?.handleKeyDown(e)) return
		},
		[],
	)

	// Resolve active agent for model resolution
	const activeDevoAgent = useMemo(() => {
		const agentName = selectedAgent ?? config?.defaultAgent
		return devoAgents?.find((a) => a.name === agentName) ?? null
	}, [selectedAgent, config?.defaultAgent, devoAgents])

	// Resolve effective model — selectedModel is seeded from the persisted project model
	// on mount/project switch (above), so it already wins at step 1 of the resolution chain.
	const effectiveModel = useMemo(
		() =>
			resolveEffectiveModel(
				selectedModel,
				activeDevoAgent,
				config?.model,
				providers?.defaults ?? {},
				providers?.providers ?? [],
				recentModels,
			),
		[selectedModel, activeDevoAgent, config?.model, providers, recentModels],
	)

	// Validate variant against the effective model's available variants.
	// Clears the variant if the current model doesn't support it (e.g. restored
	// from per-project preference but the model was changed, or provider updated).
	useEffect(() => {
		if (!selectedVariant || !effectiveModel || !providers) return
		const available = getModelVariants(
			effectiveModel.providerID,
			effectiveModel.modelID,
			providers.providers,
		)
		if (!available.includes(selectedVariant)) {
			setSelectedVariant(undefined)
		}
	}, [selectedVariant, effectiveModel, providers])

	// Model input capabilities (for attachment warnings)
	const modelCapabilities = useMemo(
		() => getModelInputCapabilities(effectiveModel, providers?.providers ?? []),
		[effectiveModel, providers],
	)

	useEffect(() => {
		if (selectedDirectory && (vcs?.state === "missing" || vcs?.state === "not_directory")) {
			setUnavailableProjectDirectories((previous) => {
				if (previous.has(selectedDirectory)) return previous
				return new Set([...previous, selectedDirectory])
			})
		}
	}, [selectedDirectory, vcs?.state])

	useEffect(() => {
		setSelectedDirectory((currentDirectory) =>
			resolveSelectedProjectDirectory(projects, projectSlug, currentDirectory, {
				preserveCurrentDirectory:
					!!manuallySelectedDirectoryRef.current &&
					manuallySelectedDirectoryRef.current === currentDirectory,
				unavailableDirectories: projectSlug ? undefined : unavailableProjectDirectories,
				lastUsedDirectory: lastProjectDirectory,
			}),
		)
	}, [projectSlug, projects, unavailableProjectDirectories, lastProjectDirectory])

	useEffect(() => {
		if (!selectedDirectory) return
		setLastProjectDirectory(selectedDirectory)
	}, [selectedDirectory, setLastProjectDirectory])

	// ---
	// Launch helpers
	// ---

	/** Persist the model + variant + agent for this project so new sessions remember it. */
	const persistProjectModel = useCallback(() => {
		if (!effectiveModel || !selectedDirectory) return
		appStore.set(setProjectModelAtom, {
			directory: selectedDirectory,
			model: {
				...effectiveModel,
				variant: selectedVariant,
				agent: selectedAgent ?? undefined,
			},
		})
	}, [effectiveModel, selectedDirectory, selectedVariant, selectedAgent])

	/** Navigate to the chat view for a given session. */
	const navigateToSession = useCallback(
		(sessionId: string) => {
			const project = projects.find((p) => p.directory === selectedDirectory)
			navigate({
				to: "/project/$projectSlug/session/$sessionId",
				params: {
					projectSlug: project?.slug ?? "unknown",
					sessionId,
				},
			})
		},
		[projects, selectedDirectory, navigate],
	)

	/** Launch a session in local mode (no worktree). */
	const launchLocal = useCallback(
		async (promptText: string, files?: FileAttachment[]) => {
			const session = await createSession(selectedDirectory)
			if (!session) return

			const currentBranch = vcs?.branch ?? ""
			if (currentBranch) {
				appStore.set(setSessionBranchAtom, { sessionId: session.id, branch: currentBranch })
			}

			persistProjectModel()
			navigateToSession(session.id)

			await sendPrompt(selectedDirectory, session.id, promptText, {
				model: effectiveModel ?? undefined,
				agent: selectedAgent ?? undefined,
				variant: selectedVariant,
				files,
			})
			clearDraft()
		},
		[
			selectedDirectory,
			createSession,
			sendPrompt,
			effectiveModel,
			selectedAgent,
			selectedVariant,
			clearDraft,
			persistProjectModel,
			navigateToSession,
			vcs,
		],
	)

	/**
	 * Launch a session in worktree mode.
	 *
	 * Creates a stub session immediately and navigates to the chat view so
	 * the user sees progress in the main content area instead of waiting
	 * on the new-chat screen. The actual worktree creation, real session
	 * creation, and prompt sending happen in the background.
	 */
	const launchWorktree = useCallback(
		(promptText: string, files?: FileAttachment[]) => {
			const sessionSlug = randomWorktreeName()

			// Create a stub session so the chat view can render immediately.
			const stubId = crypto.randomUUID()
			const now = Date.now()
			appStore.set(upsertSessionAtom, {
				session: {
					id: stubId,
					slug: sessionSlug,
					projectID: "",
					directory: selectedDirectory,
					title: "Setting up worktree...",
					version: "",
					time: { created: now, updated: now },
				},
				directory: selectedDirectory,
			})
			appStore.set(setSessionSetupPhaseAtom, {
				sessionId: stubId,
				setupPhase: "creating-worktree",
			})

			persistProjectModel()
			clearDraft()
			navigateToSession(stubId)

			// Background: create worktree -> create real session -> send prompt.
			// The chat view shows the setup phase while this runs.
			const run = async () => {
				try {
					// Phase 1: Create the worktree
					const result = await createWorktree(selectedDirectory, selectedDirectory, sessionSlug)
					const sdkDirectory = result.worktreeWorkspace

					// Phase 2: Create the real session
					appStore.set(setSessionSetupPhaseAtom, {
						sessionId: stubId,
						setupPhase: "starting-session",
					})
					const session = await createSession(sdkDirectory)
					if (!session) {
						throw new Error("Failed to create session in worktree")
					}

					// Replace the stub with the real session data. Override the
					// directory back to the parent so it groups correctly in the sidebar.
					appStore.set(upsertSessionAtom, {
						session,
						directory: selectedDirectory,
					})
					appStore.set(setSessionWorktreeAtom, {
						sessionId: session.id,
						worktreePath: result.worktreeRoot,
						worktreeBranch: result.branchName,
					})
					appStore.set(setSessionBranchAtom, {
						sessionId: session.id,
						branch: result.branchName,
					})

					// Navigate to the real session, then clean up the stub
					navigateToSession(session.id)
					appStore.set(removeSessionAtom, stubId)

					// Phase 3: Send the prompt
					await sendPrompt(sdkDirectory, session.id, promptText, {
						model: effectiveModel ?? undefined,
						agent: selectedAgent ?? undefined,
						variant: selectedVariant,
						files,
					})
				} catch (err) {
					console.error("Worktree launch failed:", err)
					// Remove the stub and navigate back to new chat
					appStore.set(removeSessionAtom, stubId)
					setError(`Worktree setup failed: ${err instanceof Error ? err.message : "Unknown error"}`)
					navigateToNewChat(navigate, projects, selectedProject?.slug, lastProjectDirectory)
				}
			}

			run()
		},
		[
			selectedDirectory,
			createSession,
			sendPrompt,
			effectiveModel,
			selectedAgent,
			selectedVariant,
			clearDraft,
			persistProjectModel,
			navigateToSession,
			navigate,
			projects,
			selectedProject,
			lastProjectDirectory,
		],
	)

	const handleLaunch = useCallback(
		async (promptText: string, files?: FileAttachment[]) => {
			if (!selectedDirectory || !promptText) return
			setLaunching(true)
			setError(null)
			try {
				if (worktreeMode === "worktree") {
					// Worktree mode navigates immediately and runs setup in the background.
					// The launching state is cleared right away since the chat view takes over.
					launchWorktree(promptText, files)
					setLaunching(false)
				} else {
					await launchLocal(promptText, files)
				}
			} catch (err) {
				setError(err instanceof Error ? err.message : "Failed to create session")
			} finally {
				setLaunching(false)
			}
		},
		[selectedDirectory, worktreeMode, launchLocal, launchWorktree],
	)

	const hasToolbar = providers

	return (
		<div className="relative flex h-full flex-col items-center justify-center px-0 py-8 sm:px-8">
			<div className="w-full max-w-3xl">
				<div className="mb-6 text-center">
					<h1 className="select-none text-[32px] font-normal leading-tight tracking-[-0.03em] text-foreground">
						What should we work on?
					</h1>
				</div>

				<div
					className="devo-composer-shell bg-background shadow-[0_8px_32px_rgba(0,0,0,0.05)]"
					data-popover-open={slashOpen || mentionOpen ? "true" : undefined}
				>
					<PromptInputProvider key={draftKey} initialInput={draft}>
						<DraftSync setDraft={setDraft} />
						<MentionBridge controllerRef={controllerRef} />
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
						<div className="relative">
							<SlashCommandPopover
								ref={slashPopoverRef}
								query={slashQuery}
								open={slashOpen}
								enabled={!launching && !!selectedDirectory}
								onSelect={handleSlashSelect}
								onClose={handleSlashClose}
							/>
							<MentionPopover
								ref={mentionPopoverRef}
								query={mentionQuery}
								open={mentionOpen}
								directory={selectedDirectory || null}
								agents={devoAgents ?? []}
								onSelect={handleMentionSelect}
								onClose={handleMentionClose}
							/>
							<PromptInput
								className="devo-composer border-border/60 bg-background/95 shadow-none"
								accept="image/png,image/jpeg,image/gif,image/webp,application/pdf"
								multiple
								maxFileSize={10 * 1024 * 1024}
								onSubmit={(message) => {
									if (message.text.trim())
										handleLaunch(
											message.text.trim(),
											message.files.length > 0 ? message.files : undefined,
										)
								}}
							>
								<PromptAttachmentPreview
									supportsImages={modelCapabilities?.image}
									supportsPdf={modelCapabilities?.pdf}
								/>
								<PromptInputTextarea
									data-prompt-input
									placeholder="Do anything"
									autoFocus
									disabled={launching || !selectedDirectory}
									className="min-h-[52px] px-4 pt-3 text-base"
									onKeyDown={handleTextareaKeyDown}
								/>

								<PromptInputFooter className="px-4 pb-2">
									<PromptInputTools>
										<AttachButton disabled={launching || !selectedDirectory} />
										{hasToolbar && (
											<PromptToolbar
												agents={devoAgents ?? []}
												selectedAgent={selectedAgent}
												defaultAgent={config?.defaultAgent}
												onSelectAgent={setSelectedAgent}
												providers={providers}
												effectiveModel={effectiveModel}
												hasModelOverride={!!selectedModel}
												onSelectModel={handleModelSelect}
												selectedVariant={selectedVariant}
												onSelectVariant={handleVariantSelect}
												disabled={launching || !selectedDirectory}
											/>
										)}
									</PromptInputTools>
									<PromptInputSubmit disabled={launching || !selectedDirectory} />
								</PromptInputFooter>
							</PromptInput>
						</div>
					</PromptInputProvider>

					<div className="px-4 py-1">
						<div className="flex min-w-0 items-center gap-2 text-sm text-muted-foreground">
							<NewChatProjectPicker
								projects={projects}
								selectedProject={selectedProject}
								selectedDirectory={selectedDirectory}
								onSelectProject={handleSelectProject}
								onStartFromScratch={startFromScratch}
								onUseExistingFolder={useExistingFolder ? handleUseExistingFolder : undefined}
							/>
							{providers && selectedDirectory && (
								<div className="min-w-0 flex-1 [&>div]:px-0 [&>div]:pt-0">
									<StatusBar
										vcs={vcs ?? null}
										isConnected={true}
										branchSlot={
											<BranchPicker
												directory={selectedDirectory}
												currentBranch={vcs?.branch}
												currentState={vcs?.state}
												onBranchChanged={handleBranchChanged}
												activeSessionCount={activeSessionCount}
											/>
										}
										extraSlot={
											vcs ? (
												<WorktreeToggle mode={worktreeMode} onModeChange={setWorktreeMode} />
											) : undefined
										}
									/>
								</div>
							)}
						</div>
					</div>

					{/* Error */}
					{error && (
						<div className="mt-2 rounded-md border border-red-500/20 bg-red-500/10 px-3 py-2 text-sm text-red-500">
							{error}
						</div>
					)}
				</div>
			</div>
		</div>
	)
}
