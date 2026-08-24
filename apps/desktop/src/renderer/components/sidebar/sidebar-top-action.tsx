import { cn } from "@devo/ui/lib/utils"
import type { ReactNode } from "react"

export const sidebarPrimaryIconClass = "size-[15px] stroke-[1.5]"

export function TopActionRow({
	children,
	icon,
	onClick,
	isActive,
}: {
	children: ReactNode
	icon: ReactNode
	onClick: () => void
	isActive?: boolean
}) {
	return (
		<button
			type="button"
			onClick={onClick}
			className={cn(
				"flex h-8 w-full items-center gap-2.5 rounded-lg px-1.5 text-left text-[13px] font-normal transition-colors",
				isActive
					? "bg-black/[0.06] text-sidebar-foreground dark:bg-white/[0.08]"
					: "text-sidebar-foreground hover:bg-black/[0.04] dark:hover:bg-white/[0.06]",
			)}
		>
			<span className="flex size-4 shrink-0 items-center justify-center text-sidebar-foreground/90">
				{icon}
			</span>
			<span className="min-w-0 flex-1 truncate">{children}</span>
		</button>
	)
}
