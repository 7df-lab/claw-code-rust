import { cn } from "@devo/ui/lib/utils"
import type { ReactNode } from "react"

/**
 * Quiet live status: muted typography only.
 * Matches Working for / transcript row type: 13px / leading-5.
 */
export function ActivityCue({
	children,
	className,
}: {
	children: ReactNode
	/** Kept for call-site clarity; live vs idle is expressed by surrounding copy. */
	active?: boolean
	className?: string
}) {
	return (
		<span
			className={cn(
				"inline-block min-w-0 text-[13px] leading-5 text-muted-foreground",
				className,
			)}
		>
			{children}
		</span>
	)
}
