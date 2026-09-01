/**
 * Compact permission-profile control for the composer footer.
 *
 * Same three presets as the TUI: Ask for approval, Approve for me, Full access.
 */

import {
	optionMenuContentClass,
	optionMenuItemClass,
} from "@devo/ui/components/option-menu-styles"
import { Popover, PopoverContent, PopoverTrigger } from "@devo/ui/components/popover"
import { cn } from "@devo/ui/lib/utils"
import { CheckIcon, ShieldCheckIcon, ShieldIcon, ShieldOffIcon } from "lucide-react"
import { useState } from "react"
import {
	COMPOSER_PERMISSION_PROFILES,
	type ComposerPermissionProfile,
	composerPermissionLabel,
} from "./composer-permission"

const TRIGGER_CLASS =
	"flex h-7 items-center gap-1.5 rounded-md border-none bg-transparent px-2 text-[13px] font-normal text-muted-foreground shadow-none transition-colors hover:bg-muted hover:text-foreground disabled:cursor-not-allowed disabled:opacity-50"

const PROFILE_ICON = {
	default: ShieldIcon,
	autoReview: ShieldCheckIcon,
	fullAccess: ShieldOffIcon,
} as const

export function ComposerPermissionPicker({
	value,
	onChange,
	disabled = false,
}: {
	value: ComposerPermissionProfile
	onChange: (profile: ComposerPermissionProfile) => void
	disabled?: boolean
}) {
	const [open, setOpen] = useState(false)
	const TriggerIcon = PROFILE_ICON[value]

	return (
		<Popover open={open} onOpenChange={setOpen}>
			<PopoverTrigger
				disabled={disabled}
				render={
					<button
						type="button"
						disabled={disabled}
						aria-label="Permission profile"
						className={TRIGGER_CLASS}
					/>
				}
			>
				<TriggerIcon className="size-3.5 shrink-0 stroke-[1.5]" aria-hidden="true" />
				<span className="max-w-[9.5rem] truncate">{composerPermissionLabel(value)}</span>
			</PopoverTrigger>
			<PopoverContent
				align="start"
				side="top"
				sideOffset={6}
				className={cn(optionMenuContentClass, "w-64 p-1")}
			>
				{COMPOSER_PERMISSION_PROFILES.map((profile) => {
					const Icon = PROFILE_ICON[profile.id]
					const selected = profile.id === value
					return (
						<button
							key={profile.id}
							type="button"
							className={cn(
								"flex w-full items-start gap-2 py-1.5 text-left transition-colors hover:bg-muted",
								optionMenuItemClass,
								"h-auto leading-snug",
								selected && "bg-muted",
							)}
							onClick={() => {
								onChange(profile.id)
								setOpen(false)
							}}
						>
							<Icon
								className="mt-0.5 size-3.5 shrink-0 stroke-[1.5] text-muted-foreground"
								aria-hidden="true"
							/>
							<span className="min-w-0 flex-1">
								<span className="block text-[13px] font-normal text-foreground">
									{profile.label}
								</span>
								<span className="mt-0.5 block text-[12px] font-normal text-muted-foreground">
									{profile.description}
								</span>
							</span>
							<span className="flex size-3.5 shrink-0 items-center justify-center" aria-hidden="true">
								{selected && <CheckIcon className="size-3.5 text-muted-foreground" />}
							</span>
						</button>
					)
				})}
			</PopoverContent>
		</Popover>
	)
}
