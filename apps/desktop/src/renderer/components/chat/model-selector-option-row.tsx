import { SearchableListPopoverItem } from "@devo/ui/components/searchable-list-popover"
import { CheckIcon } from "lucide-react"
import { MODEL_SELECTOR_ROW_INTERACTION_CLASS } from "./model-selector-row-styles"

interface ModelSelectorOptionRowProps {
	displayName: string
	providerName?: string
	reasoning: boolean
	selected: boolean
	onSelect: () => void
}

export function ModelSelectorOptionRow({
	displayName,
	providerName,
	selected,
	onSelect,
}: ModelSelectorOptionRowProps) {
	return (
		<SearchableListPopoverItem
			onSelect={onSelect}
			className={MODEL_SELECTOR_ROW_INTERACTION_CLASS}
		>
			<span
				data-slot="model-selector-check-slot"
				className="flex size-3.5 shrink-0 items-center justify-center"
				aria-hidden="true"
			>
				{selected && (
					<CheckIcon
						data-slot="model-selector-check-icon"
						className="size-3.5 text-muted-foreground"
					/>
				)}
			</span>
			<div className="min-w-0 flex-1">
				<div className="truncate">{displayName}</div>
				{providerName && (
					<div className="truncate text-[10px] text-muted-foreground/40">{providerName}</div>
				)}
			</div>
		</SearchableListPopoverItem>
	)
}
