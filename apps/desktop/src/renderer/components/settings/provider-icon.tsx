/**
 * Provider logo icons.
 *
 * Prefer locally vendored SVGs (from models.dev / Lobe Icons) so Desktop does
 * not depend on runtime CDN availability. Falls back to models.dev, then a
 * letter avatar.
 */

import { useMemo, useState } from "react"

// ============================================================
// Color palette for letter avatars (fallback)
// ============================================================

const AVATAR_COLORS = [
	"bg-blue-500/20 text-blue-400",
	"bg-purple-500/20 text-purple-400",
	"bg-green-500/20 text-green-400",
	"bg-amber-500/20 text-amber-400",
	"bg-rose-500/20 text-rose-400",
	"bg-cyan-500/20 text-cyan-400",
	"bg-indigo-500/20 text-indigo-400",
	"bg-emerald-500/20 text-emerald-400",
	"bg-orange-500/20 text-orange-400",
	"bg-pink-500/20 text-pink-400",
]

function hashString(str: string): number {
	let hash = 0
	for (let i = 0; i < str.length; i++) {
		hash = (hash << 5) - hash + str.charCodeAt(i)
		hash |= 0
	}
	return Math.abs(hash)
}

function localLogo(file: string): string {
	return new URL(`../../assets/provider-logos/${file}`, import.meta.url).href
}

/** Local logo assets keyed by provider id (and common aliases). */
const LOCAL_LOGOS: Record<string, string> = {
	alibaba: localLogo("alibaba.svg"),
	deepseek: localLogo("deepseek.svg"),
	kimi: localLogo("moonshot.svg"),
	minimax: localLogo("minimax.svg"),
	moonshot: localLogo("moonshot.svg"),
	moonshotai: localLogo("moonshotai.svg"),
	ollama: localLogo("ollama.svg"),
	openai: localLogo("openai.svg"),
	poolside: localLogo("poolside.svg"),
	qwen: localLogo("qwen.svg"),
	tencent: localLogo("tencent.svg"),
	xiaomi: localLogo("xiaomi.svg"),
	zai: localLogo("zai.svg"),
	zhipu: localLogo("zhipu.svg"),
}

/** Provider ids whose official marks are mono (black) and need dark-mode invert. */
const MONO_LOGO_IDS = new Set(["openai", "ollama"])

/** Extra remote candidates when a local asset is missing. Prefer color variants. */
const REMOTE_LOGO_FALLBACKS: Record<string, string[]> = {
	kimi: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/moonshot.svg",
		"https://models.dev/logos/moonshotai.svg",
	],
	qwen: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/qwen-color.svg",
		"https://models.dev/logos/alibaba.svg",
	],
	tencent: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/hunyuan-color.svg",
	],
	zhipu: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/zhipu-color.svg",
	],
	zai: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/chatglm-color.svg",
	],
	deepseek: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/deepseek-color.svg",
	],
	minimax: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/minimax-color.svg",
	],
	poolside: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/poolside-color.svg",
	],
	xiaomi: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/xiaomimimo.svg",
		"https://cdn.simpleicons.org/xiaomi/FF6900",
	],
	ollama: [
		"https://cdn.jsdelivr.net/npm/@lobehub/icons-static-svg@latest/icons/ollama.svg",
	],
}

function logoCandidatesFor(id: string): string[] {
	const candidates: string[] = []
	const local = LOCAL_LOGOS[id]
	if (local) candidates.push(local)
	for (const url of REMOTE_LOGO_FALLBACKS[id] ?? []) {
		if (!candidates.includes(url)) candidates.push(url)
	}
	const modelsDev = `https://models.dev/logos/${id}.svg`
	if (!candidates.includes(modelsDev)) candidates.push(modelsDev)
	return candidates
}

// ============================================================
// Component
// ============================================================

const SIZE_CLASSES = {
	xs: "size-4",
	sm: "size-7",
	md: "size-8",
	lg: "size-10",
} as const

/** Pixel sizes matching the Tailwind size classes, used for img width/height attributes */
const SIZE_PX = {
	xs: 16,
	sm: 28,
	md: 32,
	lg: 40,
} as const

interface ProviderIconProps {
	/** Provider ID (e.g. "anthropic", "openai") */
	id: string
	/** Provider display name (used for letter fallback) */
	name: string
	size?: "xs" | "sm" | "md" | "lg"
	className?: string
}

export function ProviderIcon({ id, name, size = "md", className = "" }: ProviderIconProps) {
	const candidates = useMemo(() => logoCandidatesFor(id), [id])
	const [candidateIndex, setCandidateIndex] = useState(0)

	const rounding = size === "xs" ? "rounded-sm" : "rounded-md"
	const px = SIZE_PX[size]
	const src = candidates[candidateIndex]

	if (src) {
		const mono = MONO_LOGO_IDS.has(id)
		return (
			<img
				key={src}
				src={src}
				alt={`${name} logo`}
				width={px}
				height={px}
				className={`shrink-0 object-contain ${mono ? "dark:invert" : ""} ${rounding} ${SIZE_CLASSES[size]} ${className}`}
				aria-hidden="true"
				onError={() => setCandidateIndex((index) => index + 1)}
			/>
		)
	}

	// Fallback: colored letter avatar
	const colorClass = AVATAR_COLORS[hashString(id) % AVATAR_COLORS.length]
	const letter = name.charAt(0).toUpperCase()
	const textSize = size === "xs" ? "text-[9px]" : size === "sm" ? "text-xs" : "text-sm"

	return (
		<div
			className={`flex shrink-0 items-center justify-center ${rounding} font-semibold ${SIZE_CLASSES[size]} ${textSize} ${colorClass} ${className}`}
			aria-hidden="true"
		>
			{letter}
		</div>
	)
}
