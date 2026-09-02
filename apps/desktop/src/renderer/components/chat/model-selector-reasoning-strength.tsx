import { HoverCard, HoverCardContent, HoverCardTrigger } from "@devo/ui/components/hover-card"
import {
	optionMenuContentClass,
	optionMenuItemClass,
	optionMenuSeparatorClass,
} from "@devo/ui/components/option-menu-styles"
import { Separator } from "@devo/ui/components/separator"
import { cn } from "@devo/ui/lib/utils"
import { CheckIcon, ChevronLeftIcon, ChevronRightIcon } from "lucide-react"
import { useCallback, useState } from "react"
import {
	getVariantMenuLabel,
	getVariantOptions,
	resolveSelectedVariant,
} from "./model-selector-variant-label"
import {
	MODEL_SELECTOR_ROW_INTERACTION_CLASS,
	MODEL_SELECTOR_ROW_SECONDARY_TEXT_CLASS,
} from "./model-selector-row-styles"

interface ModelSelectorVariantOptionsProps {
	variants: string[]
	selectedVariant: string | undefined
	allowDefaultVariant: boolean
	onSelectVariant: (variant: string | undefined) => void
}

export function ModelSelectorVariantOptions({
	variants,
	selectedVariant,
	allowDefaultVariant,
	onSelectVariant,
}: ModelSelectorVariantOptionsProps) {
	const resolvedVariant = resolveSelectedVariant(
		variants,
		selectedVariant,
		undefined,
		allowDefaultVariant,
	)
	const options = getVariantOptions(variants, allowDefaultVariant)

	return (
		<div className="p-1">
			{options.map((option) => {
				const selected = option.value === resolvedVariant
				return (
					<button
						key={option.key}
						type="button"
						className={cn(
							"flex w-full min-w-0 items-center text-left transition-colors",
							optionMenuItemClass,
							MODEL_SELECTOR_ROW_INTERACTION_CLASS,
							selected && "bg-muted text-foreground",
						)}
						onClick={() => onSelectVariant(option.value)}
					>
						<span
							data-slot="model-selector-variant-check-slot"
							className="flex size-3.5 shrink-0 items-center justify-center"
							aria-hidden="true"
						>
							{selected && (
								<CheckIcon
									data-slot="model-selector-variant-check-icon"
									className="size-3.5 text-muted-foreground"
								/>
							)}
						</span>
						<span className="min-w-0 flex-1 truncate">{option.label}</span>
					</button>
				)
			})}
		</div>
	)
}

interface ModelSelectorReasoningStrengthProps {
	variants: string[]
	selectedVariant: string | undefined
	allowDefaultVariant: boolean
	isMobile: boolean
	onOpenMobileView: () => void
	onSelectVariant: (variant: string | undefined) => void
	onClose: () => void
}

export function ModelSelectorReasoningStrength({
	variants,
	selectedVariant,
	allowDefaultVariant,
	isMobile,
	onOpenMobileView,
	onSelectVariant,
	onClose,
}: ModelSelectorReasoningStrengthProps) {
	const [flyoutOpen, setFlyoutOpen] = useState(false)
	const resolvedVariant = resolveSelectedVariant(
		variants,
		selectedVariant,
		undefined,
		allowDefaultVariant,
	)
	const label = getVariantMenuLabel(resolvedVariant)

	const handleSelectVariant = useCallback(
		(variant: string | undefined) => {
			onSelectVariant(variant)
			setFlyoutOpen(false)
			onClose()
		},
		[onClose, onSelectVariant],
	)

	const trigger = (
		<button
			type="button"
			className={cn(
				"group flex w-full min-w-0 items-center text-left transition-colors",
				optionMenuItemClass,
				MODEL_SELECTOR_ROW_INTERACTION_CLASS,
			)}
			aria-haspopup="menu"
			aria-expanded={isMobile ? undefined : flyoutOpen}
			onClick={isMobile ? onOpenMobileView : () => setFlyoutOpen(true)}
			onFocus={() => {
				if (!isMobile) setFlyoutOpen(true)
			}}
			onMouseEnter={() => {
				if (!isMobile) setFlyoutOpen(true)
			}}
			onKeyDown={(event) => {
				if (event.key === "ArrowRight" || event.key === "Enter") {
					event.preventDefault()
					if (isMobile) {
						onOpenMobileView()
					} else {
						setFlyoutOpen(true)
					}
				}
				if (event.key === "Escape" && flyoutOpen) {
					event.preventDefault()
					event.stopPropagation()
					setFlyoutOpen(false)
				}
			}}
		>
			<span
				data-slot="model-selector-effort-check-slot"
				className="flex size-3.5 shrink-0 items-center justify-center"
				aria-hidden="true"
			/>
			<span className="min-w-0 flex-1 truncate">Effort</span>
			<span className={cn("shrink-0 text-muted-foreground", MODEL_SELECTOR_ROW_SECONDARY_TEXT_CLASS)}>
				{label}
			</span>
			<ChevronRightIcon
				className={cn(
					"size-3.5 shrink-0 text-muted-foreground",
					MODEL_SELECTOR_ROW_SECONDARY_TEXT_CLASS,
				)}
			/>
		</button>
	)

	return (
		<div>
			<Separator className={optionMenuSeparatorClass} />
			<div className="pb-1">
				{isMobile ? (
					trigger
				) : (
					<HoverCard open={flyoutOpen} onOpenChange={setFlyoutOpen}>
						<HoverCardTrigger delay={0} closeDelay={120} render={trigger} />
						<HoverCardContent
							side="right"
							align="end"
							sideOffset={8}
							alignOffset={-4}
							className={cn("w-36 p-0", optionMenuContentClass)}
							onKeyDown={(event) => {
								if (event.key === "Escape") {
									event.preventDefault()
									event.stopPropagation()
									setFlyoutOpen(false)
								}
							}}
						>
							<ModelSelectorVariantOptions
								variants={variants}
								selectedVariant={resolvedVariant}
								allowDefaultVariant={allowDefaultVariant}
								onSelectVariant={handleSelectVariant}
							/>
						</HoverCardContent>
					</HoverCard>
				)}
			</div>
		</div>
	)
}

interface ModelSelectorReasoningStrengthMobileViewProps {
	variants: string[]
	selectedVariant: string | undefined
	allowDefaultVariant: boolean
	onBack: () => void
	onSelectVariant: (variant: string | undefined) => void
	onClose: () => void
}

export function ModelSelectorReasoningStrengthMobileView({
	variants,
	selectedVariant,
	allowDefaultVariant,
	onBack,
	onSelectVariant,
	onClose,
}: ModelSelectorReasoningStrengthMobileViewProps) {
	const handleSelectVariant = useCallback(
		(variant: string | undefined) => {
			onSelectVariant(variant)
			onClose()
		},
		[onClose, onSelectVariant],
	)

	return (
		<>
			<div className="p-1">
				<button
					type="button"
					className={cn(
						"flex w-full min-w-0 items-center text-left transition-colors",
						optionMenuItemClass,
						MODEL_SELECTOR_ROW_INTERACTION_CLASS,
					)}
					onClick={onBack}
				>
					<ChevronLeftIcon className="size-3.5 shrink-0 text-muted-foreground" />
					<span className="min-w-0 flex-1 truncate">Effort</span>
				</button>
			</div>
			<Separator className={optionMenuSeparatorClass} />
			<ModelSelectorVariantOptions
				variants={variants}
				selectedVariant={selectedVariant}
				allowDefaultVariant={allowDefaultVariant}
				onSelectVariant={handleSelectVariant}
			/>
		</>
	)
}
