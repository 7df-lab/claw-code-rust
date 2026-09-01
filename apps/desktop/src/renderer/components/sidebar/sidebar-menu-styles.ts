import {
	optionMenuContentClass,
	optionMenuItemClass,
} from "@devo/ui/components/option-menu-styles"
import { cn } from "@devo/ui/lib/utils"

export const projectMenuContentClass = cn(optionMenuContentClass, "w-[232px]")
export const sessionMenuContentClass = cn(optionMenuContentClass, "w-44")

export const rowMenuItemClass = cn(
	optionMenuItemClass,
	"cursor-default hover:bg-muted hover:text-foreground focus:bg-muted focus:text-foreground data-[highlighted]:bg-muted data-[highlighted]:text-foreground",
)
