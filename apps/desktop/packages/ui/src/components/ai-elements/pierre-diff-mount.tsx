"use client"

import {
	getHighlighterIfLoaded,
	isHighlighterLoaded,
	preloadHighlighter,
} from "@pierre/diffs"
import { MultiFileDiff, PatchDiff } from "@pierre/diffs/react"
import { useLayoutEffect, useState, type ReactNode } from "react"

type DiffOptions = {
	theme?: string
	disableLineNumbers?: boolean
	disableFileHeader?: boolean
	diffStyle?: "unified" | "split"
}

let sharedWarmup: Promise<void> | null = null

function defaultDiffWarmup(): Promise<void> {
	if (typeof window === "undefined") return Promise.resolve()
	if (sharedWarmup == null) {
		sharedWarmup = preloadHighlighter({
			themes: ["one-dark-pro", "one-light"],
			langs: ["text", "typescript", "tsx", "javascript", "jsx", "rust", "json"],
		}).catch((error: unknown) => {
			sharedWarmup = null
			throw error
		})
	}
	return sharedWarmup
}

function isHighlighterReady(): boolean {
	return isHighlighterLoaded(getHighlighterIfLoaded())
}

type PierreDiffMountProps =
	| {
			mode: "files"
			options: DiffOptions
			oldFile: { name: string; contents: string }
			newFile: { name: string; contents: string }
			warmup?: () => Promise<void>
	  }
	| {
			mode: "patch"
			options: DiffOptions
			patch: string
			warmup?: () => Promise<void>
	  }

/**
 * Mount @pierre/diffs once the shared Shiki highlighter is ready.
 * MountWhenVisible (transcript row) already waits for the collapsible panel,
 * so we do not remount here — that second mount was the main perceived delay.
 */
export function PierreDiffMount(props: PierreDiffMountProps): ReactNode {
	const [mounted, setMounted] = useState(false)
	const runWarmup = props.warmup ?? defaultDiffWarmup

	const contentFingerprint =
		props.mode === "files"
			? `${props.oldFile.name}\0${props.newFile.name}\0${props.oldFile.contents.length}\0${props.newFile.contents.length}`
			: `${props.patch.length}\0${props.patch.slice(0, 256)}`

	useLayoutEffect(() => {
		let cancelled = false

		const reveal = () => {
			if (!cancelled) setMounted(true)
		}

		setMounted(false)

		if (isHighlighterReady()) {
			reveal()
			return () => {
				cancelled = true
			}
		}

		void runWarmup().then(() => {
			if (!cancelled) reveal()
		})

		return () => {
			cancelled = true
		}
	}, [contentFingerprint, props.mode, props.options.theme, runWarmup])

	if (!mounted) {
		return <div className="min-h-px w-full" aria-hidden />
	}

	return props.mode === "files" ? (
		<MultiFileDiff
			options={props.options}
			oldFile={props.oldFile}
			newFile={props.newFile}
		/>
	) : (
		<PatchDiff options={props.options} patch={props.patch} />
	)
}
