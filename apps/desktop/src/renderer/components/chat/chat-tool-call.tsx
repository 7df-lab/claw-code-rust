import {
	CodeBlock,
	CodeBlockActions,
	CodeBlockContent,
	CodeBlockCopyButton,
	CodeBlockHeader,
	CodeBlockTitle,
} from "@devo/ui/components/ai-elements/code-block"
import { Diff, DiffContent } from "@devo/ui/components/ai-elements/diff"
import { Terminal, TerminalContent } from "@devo/ui/components/ai-elements/terminal"
import { Dialog, DialogContent, DialogTitle, DialogTrigger } from "@devo/ui/components/dialog"
import { cn } from "@devo/ui/lib/utils"

import { useSetAtom } from "jotai"
import {
	AlertTriangleIcon,
	BookOpenIcon,
	CodeIcon,
	DiffIcon,
	EditIcon,
	EyeIcon,
	FileCodeIcon,
	FileIcon,
	GlobeIcon,
	Loader2Icon,
	MinusIcon,
	PlusIcon,
	PlugIcon,
	SearchIcon,
	SquareCheckIcon,
	TerminalIcon,
	WrenchIcon,
	XIcon,
	ZapIcon,
} from "lucide-react"
import type { ReactNode } from "react"
import { memo, useCallback, useMemo } from "react"
import type { BundledLanguage } from "shiki"
import { viewFileInDiffPanelAtom } from "../../atoms/ui"
import { detectContentLanguage, detectLanguage, prettyPrintJson } from "../../lib/language"
import type { FilePart, ToolPart, ToolStateCompleted } from "../../lib/types"
import { SubAgentCard } from "./sub-agent-card"
import type { ToolCategory } from "./tool-category"
import {
	TranscriptDisclosure,
	TranscriptDisclosureContent,
	TranscriptDisclosureTrigger,
} from "./transcript-disclosure"
import {
	formatToolPathForDisplay,
	getFirstApplyPatchPath,
	shortenPathForDisplay,
	type ToolPathDisplayOptions,
} from "./tool-paths"

// ============================================================
// Constants
// ============================================================

/** Max characters to display in tool output before truncating */
const MAX_OUTPUT_LENGTH = 5000

/** Truncate output for display, preserving useful content */
function truncateOutput(output: string, max = MAX_OUTPUT_LENGTH): string {
	if (output.length <= max) return output
	return `${output.slice(0, max)}\n... (truncated)`
}

/** Shell-family tools that share the command/terminal display. */
const SHELL_TOOLS: ReadonlySet<string> = new Set(["bash", "shell_command", "exec_command"])

/**
 * The shell tool's result text is `<stdout>\n<envelope-json>`, where the
 * envelope carries display metadata ({output, command, exit, description,
 * cwd, yield_time_ms}). Strip it so the terminal shows only real command
 * output; the command and description already appear in the tool row itself.
 */
export function stripShellEnvelope(output: string): string {
	const parseEnvelope = (text: string): { body?: string } | undefined => {
		try {
			const parsed = JSON.parse(text)
			if (
				parsed &&
				typeof parsed === "object" &&
				typeof parsed.exit === "number" &&
				(typeof parsed.command === "string" || typeof parsed.cmd === "string")
			) {
				return { body: typeof parsed.output === "string" ? parsed.output : undefined }
			}
		} catch {
			// Not JSON — ordinary command output.
		}
		return undefined
	}

	const trimmed = output.trim()
	// The output IS the envelope (the command produced no stdout).
	if (trimmed.startsWith("{")) {
		const whole = parseEnvelope(trimmed)
		if (whole) return whole.body ?? ""
	}
	// Stdout followed by the envelope JSON (they are joined with "\n").
	const separator = output.lastIndexOf("\n{")
	if (separator !== -1) {
		const tail = parseEnvelope(output.slice(separator + 1).trim())
		if (tail) return output.slice(0, separator).trimEnd()
	}
	return output
}

// ============================================================
// Read output parsing — strip wrapper tags, keep original line numbers
// ============================================================

/**
 * Parses output from various read tools.
 * Handles:
 * 1. Claude Code's `cat -n` format: <file>00001| content</file>
 * 2. Devo's XML-wrapped format: <path>...</path><content>1: content</content>
 *
 * Strips the wrapper tags and trailing metadata lines, but keeps the
 * content's own line-number prefixes — the read output already carries
 * line numbers, so the UI must not add another gutter of its own.
 */
function parseReadOutput(raw: string): string {
	let text = raw

	// 1. Strip Devo XML-style tags
	text = text.replace(/<path>[\s\S]*?<\/path>\s*\n?/g, "")
	text = text.replace(/<type>[\s\S]*?<\/type>\s*\n?/g, "")

	// Extract content from <content> or <entries> if present
	const contentMatch = text.match(/<content>([\s\S]*?)<\/content>/)
	const entriesMatch = text.match(/<entries>([\s\S]*?)<\/entries>/)

	if (contentMatch) {
		text = contentMatch[1]
	} else if (entriesMatch) {
		text = entriesMatch[1]
	} else {
		// 2. Fallback: Strip <file> / </file> wrapper lines (Claude Code)
		text = text.replace(/^\s*<file>\s*\n?/, "")
		text = text.replace(/\n?\s*<\/file>\s*$/, "")
	}

	// 3. Clean up trailing metadata lines
	text = text.replace(/\n?\s*\(End of file[^)]*\)\s*$/, "")
	text = text.replace(/\n?\s*\(File has more lines[^)]*\)\s*$/, "")
	text = text.replace(/\n?\s*\(Output truncated[^)]*\)\s*$/, "")

	return text
}

// ============================================================
// Diff stats computation
// ============================================================

/**
 * Computes +additions / -deletions stats from old/new strings.
 */
function computeDiffStats(
	oldStr: string,
	newStr: string,
): { additions: number; deletions: number } {
	const oldLines = oldStr.split("\n")
	const newLines = newStr.split("\n")
	const oldSet = new Set(oldLines)
	const newSet = new Set(newLines)

	let additions = 0
	let deletions = 0

	for (const line of newLines) {
		if (!oldSet.has(line)) additions++
	}
	for (const line of oldLines) {
		if (!newSet.has(line)) deletions++
	}

	return { additions, deletions }
}

// ============================================================
// Tool info resolver
// ============================================================

export function getToolInfo(tool: string): {
	icon: typeof WrenchIcon
	title: string
} {
	switch (tool) {
		case "read":
			return { icon: EyeIcon, title: "Read" }
		case "glob":
			return { icon: SearchIcon, title: "Glob" }
		case "grep":
			return { icon: SearchIcon, title: "Grep" }
		case "list":
			return { icon: SearchIcon, title: "List" }
		case "webfetch":
			return { icon: GlobeIcon, title: "Fetch" }
		case "bash":
		case "shell_command":
		case "exec_command":
			return { icon: TerminalIcon, title: "Shell" }
		case "edit":
			return { icon: EditIcon, title: "Edit" }
		case "write":
			return { icon: FileCodeIcon, title: "Write" }
		case "apply_patch":
			return { icon: CodeIcon, title: "Patch" }
		case "task":
			return { icon: ZapIcon, title: "Agent" }
		case "todowrite":
			return { icon: SquareCheckIcon, title: "Todos" }
		case "todoread":
			return { icon: SquareCheckIcon, title: "Todos" }
		case "question":
		case "request_user_input":
			return { icon: BookOpenIcon, title: "Question" }
		default:
			if (tool.startsWith("mcp__")) {
				const segments = tool.split("__")
				const label = segments.slice(2).join("__") || segments[1] || tool
				return { icon: PlugIcon, title: `MCP · ${label}` }
			}
			return { icon: WrenchIcon, title: tool }
	}
}

/**
 * Try to extract a field value from partial JSON in `state.raw`.
 * During the `pending` state, `input` may be `{}` while the server is still
 * streaming the tool-call arguments. The `raw` field (when available) contains
 * the accumulated partial JSON string, so we can attempt to pull out early
 * fields like `command` or `description` even before the server has finished
 * parsing the full input.
 */
function extractFromRaw(state: ToolPart["state"], ...fields: string[]): string | undefined {
	if (!("raw" in state) || typeof state.raw !== "string" || !state.raw) return undefined
	const raw = state.raw
	for (const field of fields) {
		// Match "field": "value" — handles escaped quotes in the value
		const pattern = new RegExp(`"${field}"\\s*:\\s*"((?:[^"\\\\]|\\\\.)*)"`)
		const match = raw.match(pattern)
		if (match?.[1]) return match[1]
	}
	return undefined
}

/**
 * Returns a "Preparing ..." fallback label for tools in the `pending` state
 * when no meaningful subtitle could be resolved from input/raw yet.
 * Mirrors the Devo TUI's `InlineTool` behaviour (e.g. "~ Preparing write ...").
 */
function getPendingLabel(tool: string): string {
	switch (tool) {
		case "write":
			return "Preparing write..."
		case "edit":
			return "Preparing edit..."
		case "apply_patch":
			return "Preparing patch..."
		case "bash":
		case "shell_command":
		case "exec_command":
			return "Preparing command..."
		case "read":
			return "Preparing read..."
		case "task":
			return "Preparing agent..."
		case "webfetch":
			return "Preparing fetch..."
		case "question":
		case "request_user_input":
			return "Asking a question..."
		default:
			return `Preparing ${tool}...`
	}
}

/**
 * Extracts a human-readable subtitle from tool state.
 * Falls back to a "Preparing ..." label when the tool is in the `pending` state
 * and no input fields have been parsed yet (the model is still streaming arguments).
 */
export function getToolSubtitle(
	part: ToolPart,
	options: ToolPathDisplayOptions = {},
): string | undefined {
	const state = part.state
	const input = state.input
	const rawTitle = "title" in state && typeof state.title === "string" ? state.title : undefined
	const title = rawTitle && rawTitle !== part.tool && rawTitle !== part.callID ? rawTitle : undefined

	let subtitle: string | undefined

	switch (part.tool) {
		case "read":
			subtitle =
				formatToolPathForDisplay(
					(input.filePath as string | undefined) ?? (input.path as string | undefined),
					options,
				) ??
				formatToolPathForDisplay(extractFromRaw(state, "filePath", "path"), options)
			break
		case "glob":
			subtitle =
				(input.pattern as string) ??
				(input.path as string) ??
				extractFromRaw(state, "pattern", "path")
			break
		case "grep":
			subtitle =
				(input.pattern as string) ??
				(input.path as string) ??
				extractFromRaw(state, "pattern", "path")
			break
		case "bash":
		case "shell_command":
		case "exec_command":
			subtitle =
				title ??
				(input.description as string) ??
				(input.command as string) ??
				(input.cmd as string) ??
				extractFromRaw(state, "command", "cmd", "description")
			break
		case "edit":
			subtitle =
				formatToolPathForDisplay(
					(input.filePath as string | undefined) ?? (input.path as string | undefined),
					options,
				) ??
				formatToolPathForDisplay(extractFromRaw(state, "filePath", "path"), options)
			break
		case "write":
			subtitle =
				formatToolPathForDisplay(
					(input.filePath as string | undefined) ?? (input.path as string | undefined),
					options,
				) ??
				formatToolPathForDisplay(extractFromRaw(state, "filePath", "path"), options)
			break
		case "apply_patch": {
			const rawPatch = extractFromRaw(state, "patch", "diff")
			const patchPath =
				(input.filePath as string | undefined) ??
				(input.path as string | undefined) ??
				getFirstApplyPatchPath(input.patch as string | undefined) ??
				getFirstApplyPatchPath(input.diff as string | undefined) ??
				getFirstApplyPatchPath(rawPatch?.replace(/\\r\\n|\\n/g, "\n")) ??
				extractFromRaw(state, "filePath", "path")
			subtitle = formatToolPathForDisplay(patchPath, options) ?? title
			break
		}
		case "webfetch":
			subtitle = (input.url as string) ?? extractFromRaw(state, "url")
			break
		case "task":
			subtitle = (input.description as string) ?? title ?? extractFromRaw(state, "description")
			break
		case "todowrite":
		case "todoread": {
			const todos = input?.todos as Array<{ status: string }> | undefined
			if (todos && todos.length > 0) {
				const completed = todos.filter((t) => t.status === "completed").length
				subtitle = `${completed}/${todos.length} completed`
			} else {
				subtitle = title
			}
			break
		}
		case "question":
		case "request_user_input": {
			const questions = input?.questions as Array<{ question: string }> | undefined
			if (questions && questions.length > 0) {
				subtitle = questions.length === 1
					? questions[0].question
					: `${questions.length} questions`
			} else {
				subtitle = title
			}
			break
		}
		default:
			// Unknown / MCP tools: always show compact input params like [key=value, key=value]
			// Input params are more useful than the SDK-generated title for MCP tools
			subtitle = formatInputParams(input) ?? title
			break
	}

	// When pending with no resolved subtitle, show a "Preparing ..." label so
	// the user sees activity instead of a blank card (matches Devo TUI behaviour).
	if (!subtitle && state.status === "pending") {
		return getPendingLabel(part.tool)
	}

	return subtitle
}

/**
 * Formats tool input as a compact bracket notation for unknown/MCP tools.
 * e.g. { url: "https://...", format: "md" } → [url=https://..., format=md]
 */
function formatInputParams(input: Record<string, unknown>): string | undefined {
	const entries = Object.entries(input)
	if (entries.length === 0) return undefined

	const parts: string[] = []
	for (const [key, value] of entries) {
		if (value == null) continue
		const strVal = typeof value === "string" ? value : JSON.stringify(value)
		// Truncate long values
		const truncated = strVal.length > 60 ? `${strVal.slice(0, 57)}...` : strVal
		parts.push(`${key}=${truncated}`)
	}

	if (parts.length === 0) return undefined
	return `[${parts.join(", ")}]`
}

// ============================================================
// Tool-specific content renderers
// ============================================================

/**
 * Builds the text of the single terminal block shown for a bash tool call:
 * the command as its first line, followed by the (cleaned) output.
 *
 * The SDK output often echoes the command as the first line (e.g.
 * "$ command\n..."). Since we prepend the command ourselves, strip the
 * duplicate before joining.
 */
export function buildBashTerminalOutput(
	command: string | undefined,
	output: string | undefined,
	error: string | undefined,
): string {
	let body = stripShellEnvelope(error ?? output ?? "")
	if (command) {
		const prefix = `$ ${command}`
		if (body.startsWith(prefix)) {
			body = body.slice(prefix.length).replace(/^\r?\n/, "")
		}
	}
	body = truncateOutput(body)
	if (!command) return body
	return body ? `$ ${command}\n${body}` : `$ ${command}`
}

/**
 * Bash tool: one terminal block — command on the first line, ANSI-colored
 * output below. No separate command code block or "Output" header.
 *
 * During `running` state the server streams incremental output via
 * `state.metadata.output` (accumulated string, updated on every stdout/stderr
 * chunk). We read that field so the terminal updates in real-time, matching the
 * behaviour of the Devo TUI and web UI.
 */
function BashContent({ part }: { part: ToolPart }) {
	const command =
		(part.state.input?.command as string | undefined) ??
		(part.state.input?.cmd as string | undefined)

	// During "running", live output arrives in state.metadata.output.
	// After completion it moves to state.output.
	const streamingOutput =
		part.state.status === "running"
			? (part.state.metadata?.output as string | undefined)
			: undefined
	const output = part.state.status === "completed" ? part.state.output : streamingOutput
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined
	const isStreaming = part.state.status === "running"

	const terminalOutput = useMemo(
		() => buildBashTerminalOutput(command, output, error),
		[command, output, error],
	)

	if (!terminalOutput && !isStreaming) return null

	return (
		<Terminal
			output={terminalOutput}
			isStreaming={isStreaming}
			className="max-h-64 border-0 shadow-none rounded-none text-[11px]"
		>
			<TerminalContent className="max-h-56 p-3 text-[11px] leading-relaxed" />
		</Terminal>
	)
}

/**
 * Edit tool: shows inline diff of oldString → newString with syntax highlighting.
 * No inner headers — diff stats are shown in the tool row's trailing area.
 */
function EditDiffContent({ part }: { part: ToolPart }) {
	const filePath = (part.state.input?.filePath as string) ?? (part.state.input?.path as string)
	const oldString = part.state.input?.oldString as string | undefined
	const newString = part.state.input?.newString as string | undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	if (error) return <ErrorContent error={error} />

	// If we have both old and new strings, show a proper diff
	if (oldString != null && newString != null) {
		const fileName = filePath?.split("/").pop() ?? "file"
		return (
			<Diff
				mode="files"
				oldFile={{ name: fileName, content: oldString }}
				newFile={{ name: fileName, content: newString }}
				className="max-h-96 text-[11px] border-0 shadow-none rounded-none"
			>
				<DiffContent maxHeight={384} showLineNumbers hideFileHeader />
			</Diff>
		)
	}

	// Fallback: just show the output text
	const output = part.state.status === "completed" ? part.state.output : undefined
	if (output) {
		return (
			<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
				<code>{truncateOutput(output)}</code>
			</pre>
		)
	}

	return null
}

/**
 * Write tool: shows syntax-highlighted file content being written.
 * No inner header — the tool row header already shows the filename.
 */
function WriteContent({ part }: { part: ToolPart }) {
	const filePath = (part.state.input?.filePath as string) ?? (part.state.input?.path as string)
	const content = part.state.input?.content as string | undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	if (error) return <ErrorContent error={error} />

	const language = (detectLanguage(filePath) ?? "text") as BundledLanguage

	if (content) {
		const displayContent = truncateOutput(content)
		return (
			<CodeBlock
				code={displayContent}
				language={language}
				showLineNumbers
				className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
			>
				<CodeBlockContent code={displayContent} language={language} showLineNumbers />
			</CodeBlock>
		)
	}

	// Fallback to output
	const output = part.state.status === "completed" ? part.state.output : undefined
	if (output) {
		return (
			<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
				<code>{truncateOutput(output)}</code>
			</pre>
		)
	}

	return null
}

/**
 * Apply patch tool: shows diff in patch mode.
 * No inner header — the tool row header identifies this as a patch.
 */
function PatchContent({ part }: { part: ToolPart }) {
	const output = part.state.status === "completed" ? part.state.output : undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	if (error) return <ErrorContent error={error} />

	// Try to detect if output is a patch/diff
	if (output) {
		const trimmed = output.trimStart()
		const isPatch =
			trimmed.startsWith("---") || trimmed.startsWith("diff --git") || trimmed.startsWith("@@")

		if (isPatch) {
			return (
				<Diff
					mode="patch"
					patch={output}
					className="max-h-96 text-[11px] border-0 shadow-none rounded-none"
				>
					<DiffContent maxHeight={384} showLineNumbers hideFileHeader />
				</Diff>
			)
		}

		// Not a patch format — show as plain diff-highlighted text
		return (
			<CodeBlock
				code={truncateOutput(output)}
				language="diff"
				className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
			>
				<CodeBlockContent code={truncateOutput(output)} language="diff" />
			</CodeBlock>
		)
	}

	return null
}

/**
 * Read tool: shows syntax-highlighted file contents.
 * Strips wrapper tags but keeps the read output's own line numbers —
 * the code block renders no line-number gutter of its own.
 */
function ReadContent({ part }: { part: ToolPart }) {
	const filePath = (part.state.input?.filePath as string) ?? (part.state.input?.path as string)
	const output = part.state.status === "completed" ? part.state.output : undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	if (error) return <ErrorContent error={error} />
	if (!output) return null

	const language = (detectLanguage(filePath) ?? "text") as BundledLanguage

	// No inner header — the tool row header already shows the filename.
	const displayContent = truncateOutput(parseReadOutput(output))

	return (
		<CodeBlock
			code={displayContent}
			language={language}
			className="devo-read-output max-h-96 border-0 shadow-none rounded-none"
		>
			<CodeBlockContent code={displayContent} language={language} />
		</CodeBlock>
	)
}

/** Search tools (glob/grep/list): shows results; pattern stays in the row subtitle. */
function SearchContent({ part }: { part: ToolPart }) {
	const pattern = (part.state.input?.pattern as string) ?? undefined
	const include = (part.state.input?.include as string) ?? (part.state.input?.glob as string) ?? undefined
	const path = (part.state.input?.path as string) ?? undefined
	const output = part.state.status === "completed" ? part.state.output : undefined
	// Grep/Glob already put `pattern` in the tool-row subtitle ("Grep · …").
	const patternInSubtitle = part.tool === "grep" || part.tool === "glob"
	const showPattern = Boolean(pattern) && !patternInSubtitle
	const hasMeta = showPattern || Boolean(include) || Boolean(path)

	return (
		<div className="space-y-1.5 px-3.5 py-2.5">
			{hasMeta && (
				<div className="flex flex-wrap gap-x-3 gap-y-0.5 text-xs text-muted-foreground/70">
					{showPattern && (
						<span>
							pattern: <span className="font-mono text-foreground/60">{pattern}</span>
						</span>
					)}
					{include && (
						<span>
							include: <span className="font-mono text-foreground/60">{include}</span>
						</span>
					)}
					{path && (
						<span>
							in: <span className="font-mono text-foreground/60">{shortenPathForDisplay(path)}</span>
						</span>
					)}
				</div>
			)}
			{output && (
				<pre className="max-h-48 overflow-auto rounded bg-muted/40 px-2 py-1 font-mono text-[11px] text-muted-foreground whitespace-pre">
					<code>{truncateOutput(output)}</code>
				</pre>
			)}
		</div>
	)
}

/** WebFetch tool: shows URL + fetched content with optional markdown/json highlighting */
function WebFetchContent({ part }: { part: ToolPart }) {
	const url = part.state.input?.url as string | undefined
	const format = part.state.input?.format as string | undefined
	const output = part.state.status === "completed" ? part.state.output : undefined

	const language = useMemo(() => {
		if (format === "html") return "html"
		if (format === "json") return "json"
		if (output) return detectContentLanguage(output)
		return undefined
	}, [format, output]) as BundledLanguage | undefined

	const displayOutput = useMemo(() => {
		if (!output) return undefined
		if (language === "json") return prettyPrintJson(output)
		return output
	}, [output, language])

	return (
		<div className="space-y-1.5">
			{url && (
				<div className="truncate px-3.5 pt-2.5 font-mono text-xs text-muted-foreground/70">
					{url}
				</div>
			)}
			{displayOutput && language ? (
				<CodeBlock
					code={truncateOutput(displayOutput)}
					language={language}
					className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
				>
					<CodeBlockContent code={truncateOutput(displayOutput)} language={language} />
				</CodeBlock>
			) : output ? (
				<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
					<code>{truncateOutput(output)}</code>
				</pre>
			) : null}
		</div>
	)
}

/** TodoWrite tool: shows checklist items */
function TodoContent({ part }: { part: ToolPart }) {
	const todos =
		(part.state.input?.todos as Array<{ content: string; status: string }> | undefined) ?? []

	if (todos.length === 0) return null

	return (
		<div className="space-y-1 px-3.5 py-2.5">
			{todos.map((todo, i) => (
				<div
					key={`todo-${todo.content.slice(0, 20)}-${i}`}
					className="flex items-start gap-2 text-xs"
				>
					<span className="mt-0.5">
						{todo.status === "completed" ? (
							<SquareCheckIcon className="size-3.5 text-green-500" />
						) : todo.status === "in_progress" ? (
							<Loader2Icon className="size-3.5 animate-spin text-blue-400" />
						) : todo.status === "cancelled" ? (
							<SquareCheckIcon className="size-3.5 text-muted-foreground/40" />
						) : (
							<span className="inline-block size-3.5 rounded-sm border border-border" />
						)}
					</span>
					<span
						className={cn(
							todo.status === "completed"
								? "text-muted-foreground line-through"
								: todo.status === "cancelled"
									? "text-muted-foreground/50 line-through"
									: todo.status === "in_progress"
										? "text-foreground"
										: "text-foreground/80",
						)}
					>
						{todo.content}
					</span>
				</div>
			))}
		</div>
	)
}

/** Error content for any tool */
function ErrorContent({ error }: { error: string }) {
	return (
		<div className="mx-3.5 my-2.5 flex items-start gap-2 rounded bg-muted/30 px-2 py-1.5 text-xs text-muted-foreground">
			<AlertTriangleIcon className="mt-0.5 size-3 shrink-0" aria-hidden="true" />
			<pre className="max-h-32 overflow-auto font-mono">
				<code>{error.length > 500 ? `${error.slice(0, 500)}...` : error}</code>
			</pre>
		</div>
	)
}

/**
 * Generic tool output: auto-detects JSON and other structured content
 * for syntax highlighting, falls back to plain text.
 */
function GenericContent({ part }: { part: ToolPart }) {
	const output = part.state.status === "completed" ? part.state.output : undefined
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined

	const language = useMemo(() => {
		if (!output) return undefined
		return detectContentLanguage(output) as BundledLanguage | undefined
	}, [output])

	const displayOutput = useMemo(() => {
		if (!output) return undefined
		if (language === "json") return prettyPrintJson(output)
		return output
	}, [output, language])

	return (
		<div>
			{displayOutput && language ? (
				<CodeBlock
					code={truncateOutput(displayOutput)}
					language={language}
					className="max-h-96 border-0 shadow-none rounded-none text-[11px]"
				>
					<CodeBlockHeader className="px-3 py-1.5">
						<CodeBlockTitle className="text-[11px]">
							<span className="uppercase text-muted-foreground">{language}</span>
						</CodeBlockTitle>
						<CodeBlockActions>
							<CodeBlockCopyButton className="size-6" />
						</CodeBlockActions>
					</CodeBlockHeader>
					<CodeBlockContent code={truncateOutput(displayOutput)} language={language} />
				</CodeBlock>
			) : output ? (
				<pre className="max-h-48 overflow-auto px-3.5 py-2.5 font-mono text-[11px] text-muted-foreground">
					<code>{truncateOutput(output)}</code>
				</pre>
			) : null}
			{error && <ErrorContent error={error} />}
		</div>
	)
}

// ============================================================
// Tool group summaries
// ============================================================

/**
 * Verb for an explore tool group: specific when every tool in the group is
 * the same kind (e.g. a run of greps reads "Searched", not "Read").
 */
function exploreGroupVerb(tools: ToolPart[]): string {
	const first = tools[0]?.tool
	if (!first || !tools.every((t) => t.tool === first)) return "Explored"
	switch (first) {
		case "read":
			return "Read"
		case "grep":
		case "glob":
			return "Searched"
		case "list":
			return "Listed"
		default:
			return "Explored"
	}
}

export function describeToolGroup(
	category: ToolCategory,
	tools: ToolPart[],
	projectRoot?: string | null,
): string {
	const count = tools.length

	if (count <= 3) {
		const details = tools
			.map((t) => getToolSubtitle(t, { projectRoot }))
			.filter(Boolean)
			.map((s) => {
				const parts = s!.split("/")
				return parts.length > 1 ? parts[parts.length - 1] : s
			})

		if (details.length > 0) {
			switch (category) {
				case "explore":
					return `${exploreGroupVerb(tools)} ${count === 1 ? details[0] : details.join(", ")}`
				case "edit":
					return count === 1 ? `Edited ${details[0]}` : `Edited ${details.join(", ")}`
				case "run":
					return count === 1 ? `Ran ${details[0]}` : `Ran ${count} commands`
				case "delegate":
					return count === 1 ? `Delegated: ${details[0]}` : `Delegated ${count} tasks`
				case "fetch":
					return count === 1 ? `Fetched ${details[0]}` : `Fetched ${count} URLs`
				case "ask":
					return "Asked a question"
				case "plan":
					return "Updated plan"
				default:
					return `Ran ${details.join(", ")}`
			}
		}
	}

	switch (category) {
		case "explore": {
			const fileCount = tools.filter((t) => t.tool === "read" || t.tool === "list").length
			const searchCount = tools.filter((t) => t.tool === "grep" || t.tool === "glob").length
			if (fileCount > 0 && searchCount > 0) {
				return `Explored ${fileCount} ${fileCount === 1 ? "file" : "files"}, ${searchCount} ${searchCount === 1 ? "search" : "searches"}`
			}
			const verb = exploreGroupVerb(tools)
			if (verb === "Searched") {
				return `Searched ${count} ${count === 1 ? "pattern" : "patterns"}`
			}
			return `${verb} ${count} ${count === 1 ? "file" : "files"}`
		}
		case "edit":
			return `Edited ${count} files`
		case "run":
			return `Ran ${count} commands`
		case "delegate":
			return `Delegated ${count} tasks`
		case "fetch":
			return `Fetched ${count} URLs`
		case "ask":
			return `Asked ${count} questions`
		case "plan":
			return "Updated plan"
		default:
			return `Ran ${count} tools`
	}
}

export function isGroupRunning(tools: ToolPart[], turnWorking = true): boolean {
	if (!turnWorking) return false
	return tools.some((t) => t.state.status === "running" || t.state.status === "pending")
}

export function isGroupError(tools: ToolPart[]): boolean {
	return tools.some((t) => t.state.status === "error")
}

/**
 * Returns whether a tool has expandable content.
 */
function hasExpandableContent(part: ToolPart): boolean {
	const { tool, state } = part
	// Task uses SubAgentCard, not a tool row
	if (tool === "task") return false
	// Todowrite has expandable todo items
	if (tool === "todowrite" || tool === "todoread") {
		const todos = state.input?.todos as Array<{ content: string; status: string }> | undefined
		return (todos?.length ?? 0) > 0
	}
	// Edit tool has content if we have old/new strings
	if (tool === "edit") {
		const oldString = state.input?.oldString as string | undefined
		const newString = state.input?.newString as string | undefined
		if (oldString != null && newString != null) return true
	}
	// Write tool has content if we have the file content
	if (tool === "write") {
		const content = state.input?.content as string | undefined
		if (content) return true
	}
	// If there's output or error, there's content
	if (state.status === "completed" && state.output) return true
	if (state.status === "error") return true
	// Shell tools always have content (the command at least)
	if (SHELL_TOOLS.has(tool)) return true
	return false
}

/**
 * Resolves the content renderer for a tool.
 */
function getToolContent(part: ToolPart): ReactNode {
	const error = part.state.status === "error" ? (part.state as { error: string }).error : undefined
	if (
		error &&
		!SHELL_TOOLS.has(part.tool) &&
		part.tool !== "edit" &&
		part.tool !== "write" &&
		part.tool !== "read" &&
		part.tool !== "apply_patch"
	) {
		return <ErrorContent error={error} />
	}

	switch (part.tool) {
		case "bash":
		case "shell_command":
		case "exec_command":
			return <BashContent part={part} />
		case "edit":
			return <EditDiffContent part={part} />
		case "write":
			return <WriteContent part={part} />
		case "apply_patch":
			return <PatchContent part={part} />
		case "read":
			return <ReadContent part={part} />
		case "glob":
		case "grep":
		case "list":
			return <SearchContent part={part} />
		case "webfetch":
			return <WebFetchContent part={part} />
		case "todowrite":
		case "todoread":
			return <TodoContent part={part} />
		default:
			return <GenericContent part={part} />
	}
}

// ============================================================
// ChatToolCall — main export
// ============================================================

interface ChatToolCallProps {
	part: ToolPart
	/** Whether the turn containing this tool has an error (enables delete action) */
	turnHasError?: boolean
	/** When false, running/pending tools do not show a live spinner. */
	turnWorking?: boolean
	/** Delete this tool part (for error recovery) */
	onDelete?: (part: ToolPart) => void
	/** Project root used only for display-only path labels. */
	projectRoot?: string | null
	/** Tighter row rhythm for nested items inside a tool group. */
	compact?: boolean
	open?: boolean
	defaultOpen?: boolean
	onOpenChange?: (open: boolean) => void
}

/**
 * Compares two ToolPart objects for meaningful changes.
 * Avoids re-renders when a new object reference has the same content.
 */
function areToolPartsEqual(a: ToolPart, b: ToolPart): boolean {
	if (a === b) return true
	if (a.id !== b.id) return false
	if (a.state.status !== b.state.status) return false
	// During "pending", the server streams partial tool-call arguments into
	// state.raw. Compare raw length so the subtitle updates as arguments arrive.
	if (a.state.status === "pending" && b.state.status === "pending") {
		if (a.state.raw.length !== b.state.raw.length) return false
	}
	// During "running", the server streams incremental output via
	// state.metadata.output. Compare the accumulated output length so
	// React re-renders the terminal on every new chunk.
	if (a.state.status === "running" && b.state.status === "running") {
		const aMeta = a.state.metadata?.output as string | undefined
		const bMeta = b.state.metadata?.output as string | undefined
		if ((aMeta?.length ?? 0) !== (bMeta?.length ?? 0)) return false
	}
	// Compare output/error lengths for completed/error states
	if (a.state.status === "completed" && b.state.status === "completed") {
		if (a.state.output.length !== b.state.output.length) return false
		if (a.state.time.end !== b.state.time.end) return false
	}
	if (a.state.status === "error" && b.state.status === "error") {
		if (a.state.error !== b.state.error) return false
	}
	return true
}

/**
 * Renders a single tool call as a unified disclosure row with tool-specific
 * content, or as a SubAgentCard for sub-agent tasks.
 */
export const ChatToolCall = memo(
	function ChatToolCall({
		part,
		turnHasError = false,
		turnWorking = true,
		onDelete,
		projectRoot,
		compact = false,
		open,
		defaultOpen: defaultOpenProp,
		onOpenChange,
	}: ChatToolCallProps) {
		const viewFileInDiff = useSetAtom(viewFileInDiffPanelAtom)

		// Compute diff stats for edit tools (shown in trailing area)
		// Must be called before early returns to satisfy hooks rules.
		const diffStats = useMemo(() => {
			if (part.tool !== "edit") return undefined
			const oldString = part.state.input?.oldString as string | undefined
			const newString = part.state.input?.newString as string | undefined
			if (oldString == null || newString == null) return undefined
			return computeDiffStats(oldString, newString)
		}, [part.tool, part.state.input?.oldString, part.state.input?.newString])

		// Extract file path for edit-category tools (used for "View diff" button)
		const editFilePath = useMemo(() => {
			if (part.tool !== "edit" && part.tool !== "write" && part.tool !== "apply_patch") {
				return undefined
			}
			return (part.state.input?.filePath as string) ?? (part.state.input?.path as string)
		}, [part.tool, part.state.input?.filePath, part.state.input?.path])

		const handleViewDiff = useCallback(
			(e: React.MouseEvent) => {
				e.stopPropagation()
				if (editFilePath) viewFileInDiff(editFilePath)
			},
			[editFilePath, viewFileInDiff],
		)

		const status = part.state.status as "running" | "error" | "completed" | "pending"
		const isRunning = turnWorking && (status === "running" || status === "pending")

		// Build trailing element: diff stats, or a running spinner.
		const trailingElement = useMemo(() => {
			const parts: ReactNode[] = []

			if (diffStats) {
				parts.push(
					<span key="stats" className="flex items-center gap-1.5 text-[11px]">
						<span className="flex items-center gap-0.5 text-diff-addition-foreground">
							<PlusIcon className="size-2.5" aria-hidden="true" />
							{diffStats.additions}
						</span>
						<span className="flex items-center gap-0.5 text-diff-deletion-foreground">
							<MinusIcon className="size-2.5" aria-hidden="true" />
							{diffStats.deletions}
						</span>
					</span>,
				)
			}

			if (isRunning) {
				parts.push(
					<Loader2Icon
						key="running"
						className="size-3 animate-spin text-muted-foreground/40"
					/>,
				)
			}

			if (parts.length === 0) return undefined
			if (parts.length === 1) return parts[0]
			return <span className="flex items-center gap-2.5">{parts}</span>
		}, [diffStats, isRunning])

		// When the turn has an error, add a delete button so the user can
		// surgically remove a problematic tool part and continue the conversation.
		const handleDelete = useCallback(
			(e: React.MouseEvent) => {
				e.stopPropagation()
				onDelete?.(part)
			},
			[onDelete, part],
		)

		const finalTrailing = useMemo(() => {
			if (!turnHasError || !onDelete) return trailingElement
			const deleteButton = (
				<button
					key="delete-part"
					type="button"
					onClick={handleDelete}
					className="rounded p-0.5 text-muted-foreground/40 transition-colors hover:bg-red-500/20 hover:text-red-400"
					title="Remove this tool call to recover from the error"
				>
					<XIcon className="size-3" aria-hidden="true" />
				</button>
			)
			if (!trailingElement) return deleteButton
			return (
				<span className="flex items-center gap-2">
					{trailingElement}
					{deleteButton}
				</span>
			)
		}, [turnHasError, onDelete, trailingElement, handleDelete])

		// Skip rendering todoread parts without output
		if (part.tool === "todoread" && part.state.status !== "completed") return null

		// --- Task tool: Sub-agent card ---
		if (part.tool === "task") {
			return <SubAgentCard part={part} />
		}

		// --- All other tools (including todos): unified tool row ---
		const { title } = getToolInfo(part.tool)
		const subtitle = getToolSubtitle(part, { projectRoot })
		const hasContent = hasExpandableContent(part)
		const defaultOpen = defaultOpenProp ?? false

		// Extract attachments
		const attachments: FilePart[] =
			part.state.status === "completed"
				? ((part.state as ToolStateCompleted).attachments ?? [])
				: []

		const label = subtitle ? (
			<>
				<span>{title}</span>
				<span className="text-muted-foreground/60"> · {subtitle}</span>
			</>
		) : (
			<span>{title}</span>
		)

		// Completed edit-category tools can jump to the diff panel; the action
		// lives at the top of the expanded content so the row trailing stays lean.
		const showViewDiff = editFilePath != null && status === "completed"

		return (
			<div className={attachments.length > 0 && !compact ? "space-y-1.5" : undefined}>
				<TranscriptDisclosure
					defaultOpen={defaultOpen}
					expandable={hasContent}
					open={open}
					onOpenChange={onOpenChange}
				>
					<TranscriptDisclosureTrigger
						className={compact ? "py-0" : undefined}
						label={label}
						trailing={finalTrailing}
					/>
					{hasContent && (
						<TranscriptDisclosureContent rail className="overflow-hidden">
							{showViewDiff && (
								<div className="flex justify-end pb-1">
									<button
										type="button"
										onClick={handleViewDiff}
										className="inline-flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-muted-foreground/50 transition-colors hover:bg-muted hover:text-foreground"
										title="View in diff panel"
									>
										<DiffIcon className="size-3" aria-hidden="true" />
										View diff
									</button>
								</div>
							)}
							{getToolContent(part)}
						</TranscriptDisclosureContent>
					)}
				</TranscriptDisclosure>

				{attachments.length > 0 && <ToolAttachments attachments={attachments} />}
			</div>
		)
	},
	(prev, next) => {
		if (!areToolPartsEqual(prev.part, next.part)) return false
		// open is controlled by the parent timeline (expandedRowIds); without this
		// comparison the memo blocks the re-render and the row can never expand.
		if (prev.open !== next.open) return false
		if (prev.compact !== next.compact) return false
		if (prev.turnHasError !== next.turnHasError) return false
		if (prev.turnWorking !== next.turnWorking) return false
		if (prev.projectRoot !== next.projectRoot) return false
		// onDelete/onOpenChange are callback refs - skip reference comparison to avoid
		// re-renders from parent creating new closures
		return true
	},
)

// ============================================================
// ToolAttachments — inline thumbnails for tool output images
// ============================================================

function ToolAttachments({ attachments }: { attachments: FilePart[] }) {
	const imageAttachments = attachments.filter((a) => a.mime.startsWith("image/"))
	const otherAttachments = attachments.filter((a) => !a.mime.startsWith("image/"))

	if (imageAttachments.length === 0 && otherAttachments.length === 0) return null

	return (
		<div className="ml-6 flex flex-wrap gap-2">
			{imageAttachments.map((file) => (
				<Dialog key={file.id}>
					<DialogTrigger
						render={
							<button
								type="button"
								className="group/att relative size-12 shrink-0 overflow-hidden rounded border border-border bg-muted transition-colors hover:border-muted-foreground/30"
							/>
						}
					>
						<img
							src={file.url}
							alt={file.filename ?? "Tool output image"}
							className="size-full object-cover"
						/>
					</DialogTrigger>
					<DialogContent className="max-h-[90vh] max-w-4xl overflow-auto p-0">
						<DialogTitle className="sr-only">{file.filename ?? "Tool output preview"}</DialogTitle>
						<img
							src={file.url}
							alt={file.filename ?? "Tool output image"}
							className="max-h-[85vh] w-full object-contain"
						/>
					</DialogContent>
				</Dialog>
			))}
			{otherAttachments.map((file) => (
				<div
					key={file.id}
					className="flex items-center gap-1 rounded border border-border bg-muted px-2 py-1 text-[11px] text-muted-foreground"
				>
					<FileIcon className="size-3" aria-hidden="true" />
					<span className="max-w-[120px] truncate">{file.filename ?? file.mime}</span>
				</div>
			))}
		</div>
	)
}
