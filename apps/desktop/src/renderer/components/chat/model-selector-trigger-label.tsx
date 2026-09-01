import { cn } from "@devo/ui/lib/utils"

interface ModelSelectorTriggerLabelProps {
	displayName: string
	variantLabel?: string | null
}

export function ModelSelectorTriggerLabel({
	displayName,
	variantLabel,
}: ModelSelectorTriggerLabelProps) {
	return (
		<span className="flex min-w-0 items-center gap-1.5">
			<span
				data-slot="model-selector-trigger-model"
				className="min-w-0 truncate text-[13px] font-normal text-muted-foreground"
			>
				{displayName}
			</span>
			{variantLabel && (
				<span
					data-slot="model-selector-trigger-variant"
					className={cn("shrink-0 text-[12px] font-normal text-muted-foreground/50")}
				>
					{variantLabel}
				</span>
			)}
		</span>
	)
}
