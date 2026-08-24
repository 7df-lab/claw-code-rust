"use client"

import { Button } from "@devo/ui/components/button"
import { cn } from "@devo/ui/lib/utils"
import Ansi from "ansi-to-react"
import { CheckIcon, CopyIcon, TerminalIcon, Trash2Icon } from "lucide-react"
import type { ComponentProps, HTMLAttributes } from "react"
import { createContext, memo, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react"

import { Shimmer } from "./shimmer"

interface TerminalContextType {
	output: string
	isStreaming: boolean
	autoScroll: boolean
	onClear?: () => void
}

const TerminalContext = createContext<TerminalContextType>({
	autoScroll: true,
	isStreaming: false,
	output: "",
})

export type TerminalProps = HTMLAttributes<HTMLDivElement> & {
	output: string
	isStreaming?: boolean
	autoScroll?: boolean
	onClear?: () => void
}

export const Terminal = ({
	output,
	isStreaming = false,
	autoScroll = true,
	onClear,
	className,
	children,
	...props
}: TerminalProps) => {
	const contextValue = useMemo(
		() => ({ autoScroll, isStreaming, onClear, output }),
		[autoScroll, isStreaming, onClear, output],
	)

	return (
		<TerminalContext.Provider value={contextValue}>
			<div
				className={cn(
					"flex flex-col overflow-hidden rounded-lg border border-border bg-background text-foreground",
					className,
				)}
				{...props}
			>
				{children ?? (
					<>
						<TerminalHeader>
							<TerminalTitle />
							<div className="flex items-center gap-1">
								<TerminalStatus />
								<TerminalActions>
									<TerminalCopyButton />
									{onClear && <TerminalClearButton />}
								</TerminalActions>
							</div>
						</TerminalHeader>
						<TerminalContent />
					</>
				)}
			</div>
		</TerminalContext.Provider>
	)
}

export type TerminalHeaderProps = HTMLAttributes<HTMLDivElement>

export const TerminalHeader = ({ className, children, ...props }: TerminalHeaderProps) => (
	<div
		className={cn(
			"flex items-center justify-between border-border border-b bg-muted/40 px-4 py-2",
			className,
		)}
		{...props}
	>
		{children}
	</div>
)

export type TerminalTitleProps = HTMLAttributes<HTMLDivElement>

export const TerminalTitle = ({ className, children, ...props }: TerminalTitleProps) => (
	<div
		className={cn("flex items-center gap-2 text-muted-foreground text-sm", className)}
		{...props}
	>
		<TerminalIcon className="size-4" />
		{children ?? "Terminal"}
	</div>
)

export type TerminalStatusProps = HTMLAttributes<HTMLDivElement>

export const TerminalStatus = ({ className, children, ...props }: TerminalStatusProps) => {
	const { isStreaming } = useContext(TerminalContext)

	if (!isStreaming) {
		return null
	}

	return (
		<div
			className={cn("flex items-center gap-2 text-muted-foreground text-xs", className)}
			{...props}
		>
			{children ?? <Shimmer className="w-16">Running...</Shimmer>}
		</div>
	)
}

export type TerminalActionsProps = HTMLAttributes<HTMLDivElement>

export const TerminalActions = ({ className, children, ...props }: TerminalActionsProps) => (
	<div className={cn("flex items-center gap-1", className)} {...props}>
		{children}
	</div>
)

export type TerminalCopyButtonProps = ComponentProps<typeof Button> & {
	onCopy?: () => void
	onError?: (error: Error) => void
	timeout?: number
}

export const TerminalCopyButton = ({
	onCopy,
	onError,
	timeout = 2000,
	children,
	className,
	...props
}: TerminalCopyButtonProps) => {
	const [isCopied, setIsCopied] = useState(false)
	const timeoutRef = useRef<number>(0)
	const { output } = useContext(TerminalContext)

	const copyToClipboard = useCallback(async () => {
		if (typeof window === "undefined" || !navigator?.clipboard?.writeText) {
			onError?.(new Error("Clipboard API not available"))
			return
		}

		try {
			await navigator.clipboard.writeText(output)
			setIsCopied(true)
			onCopy?.()
			timeoutRef.current = window.setTimeout(() => setIsCopied(false), timeout)
		} catch (error) {
			onError?.(error as Error)
		}
	}, [output, onCopy, onError, timeout])

	useEffect(
		() => () => {
			window.clearTimeout(timeoutRef.current)
		},
		[],
	)

	const Icon = isCopied ? CheckIcon : CopyIcon

	return (
		<Button
			className={cn(
				"size-7 shrink-0 text-muted-foreground hover:bg-muted hover:text-foreground",
				className,
			)}
			onClick={copyToClipboard}
			size="icon"
			variant="ghost"
			{...props}
		>
			{children ?? <Icon size={14} />}
		</Button>
	)
}

export type TerminalClearButtonProps = ComponentProps<typeof Button>

export const TerminalClearButton = ({
	children,
	className,
	...props
}: TerminalClearButtonProps) => {
	const { onClear } = useContext(TerminalContext)

	if (!onClear) {
		return null
	}

	return (
		<Button
			className={cn(
				"size-7 shrink-0 text-muted-foreground hover:bg-muted hover:text-foreground",
				className,
			)}
			onClick={onClear}
			size="icon"
			variant="ghost"
			{...props}
		>
			{children ?? <Trash2Icon size={14} />}
		</Button>
	)
}

export type TerminalContentProps = HTMLAttributes<HTMLDivElement>

/**
 * Renders ANSI output as append-only committed lines plus a live tail.
 * Re-parsing the full blob on every streaming chunk remounts the ANSI tree
 * and makes the shell tool view flash in the Desktop transcript.
 */
function TerminalAnsiOutput({ output, isStreaming }: { output: string; isStreaming: boolean }) {
	const lines = output.split("\n")
	const committedCount = isStreaming ? Math.max(0, lines.length - 1) : lines.length
	const tail = isStreaming ? (lines[lines.length - 1] ?? "") : null

	return (
		<pre className="whitespace-pre-wrap break-words">
			{lines.slice(0, committedCount).map((line, index) => (
				<AnsiLine key={index} text={line} />
			))}
			{tail !== null && <AnsiLine live text={tail} />}
			{isStreaming && (
				<span className="ml-0.5 inline-block h-4 w-2 animate-pulse bg-foreground" />
			)}
		</pre>
	)
}

const AnsiLine = memo(function AnsiLine({ live = false, text }: { live?: boolean; text: string }) {
	return (
		<span>
			<Ansi>{text}</Ansi>
			{live ? null : "\n"}
		</span>
	)
})

export const TerminalContent = ({ className, children, ...props }: TerminalContentProps) => {
	const { output, isStreaming, autoScroll } = useContext(TerminalContext)
	const containerRef = useRef<HTMLDivElement>(null)

	useEffect(() => {
		if (autoScroll && containerRef.current) {
			containerRef.current.scrollTop = containerRef.current.scrollHeight
		}
	}, [output, autoScroll])

	return (
		<div
			className={cn("max-h-96 overflow-auto p-4 font-mono text-sm leading-relaxed", className)}
			ref={containerRef}
			{...props}
		>
			{children ?? <TerminalAnsiOutput isStreaming={isStreaming} output={output} />}
		</div>
	)
}
