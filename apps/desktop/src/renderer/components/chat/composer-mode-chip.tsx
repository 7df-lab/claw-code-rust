import { Tooltip, TooltipContent, TooltipTrigger } from "@devo/ui/components/tooltip"
import { cn } from "@devo/ui/lib/utils"
import { GoalIcon, ListTodoIcon, XIcon } from "lucide-react"

export type ComposerModeChipVariant = "plan" | "goal"

const CHIP_CONFIG = {
	plan: {
		icon: ListTodoIcon,
		label: "Plan",
		description: "Plan mode — the agent will propose a plan before building",
		tooltipExtra: "Shift + Tab to toggle",
	},
	goal: {
		icon: GoalIcon,
		label: "Goal",
		description: "Goal — the next message sets a session goal",
		tooltipExtra: null,
	},
} as const

export function ComposerModeChip({
	variant,
	onRemove,
	disabled = false,
}: {
	variant: ComposerModeChipVariant
	onRemove: () => void
	disabled?: boolean
}) {
	const config = CHIP_CONFIG[variant]
	const Icon = config.icon
	const isPlan = variant === "plan"

	return (
		<Tooltip>
			<TooltipTrigger
				render={
					<div
						className={cn(
							"group inline-flex h-7 items-center gap-1 rounded-full px-2 text-xs transition-colors",
							isPlan
								? "bg-amber-500/12 text-amber-800 hover:bg-amber-500/18 dark:text-amber-300"
								: "text-muted-foreground hover:bg-muted hover:text-foreground",
							disabled && "pointer-events-none opacity-50",
						)}
					/>
				}
			>
				<button
					type="button"
					aria-label={`Remove ${config.label}`}
					disabled={disabled}
					onClick={onRemove}
					className="pointer-events-none relative inline-flex size-3.5 shrink-0 items-center justify-center transition-colors group-focus-within:pointer-events-auto group-hover:pointer-events-auto focus-visible:rounded-sm focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
				>
					<Icon
						className="size-3.5 stroke-[1.5] opacity-100 transition-opacity group-focus-within:opacity-0 group-hover:opacity-0"
						aria-hidden="true"
					/>
					<XIcon
						className="absolute size-3.5 stroke-[1.5] opacity-0 transition-opacity group-focus-within:opacity-100 group-hover:opacity-100"
						aria-hidden="true"
					/>
				</button>
				<span className="font-medium">{config.label}</span>
			</TooltipTrigger>
			<TooltipContent side="top" align="start" className="max-w-[220px]">
				<div className="text-xs">{config.description}</div>
				{config.tooltipExtra ? (
					<div className="mt-0.5 text-[11px] text-muted-foreground">{config.tooltipExtra}</div>
				) : null}
			</TooltipContent>
		</Tooltip>
	)
}
