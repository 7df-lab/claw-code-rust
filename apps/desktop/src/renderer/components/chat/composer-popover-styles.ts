import { cn } from "@devo/ui/lib/utils"

/** Shared shell for composer `@` / `/` suggestion popovers. */
export const composerPopoverShellClass =
	"absolute inset-x-0 bottom-full z-50 mb-1.5 origin-bottom-left overflow-hidden rounded-lg border border-border/70 bg-popover shadow-sm"

export const composerPopoverScrollClass = "max-h-64 overflow-y-auto overscroll-contain"

export const composerPopoverListClass = "flex flex-col gap-0.5 p-1"

export const composerPopoverHeaderClass =
	"flex items-center gap-2 border-b border-border/50 px-2.5 py-2"

export const composerPopoverGroupLabelClass =
	"sticky top-0 z-10 bg-popover px-2.5 py-1.5 text-[11px] font-medium tracking-normal text-muted-foreground/70"

export const composerPopoverEmptyClass =
	"px-2.5 py-6 text-center text-[13px] leading-5 text-muted-foreground/70"

export const composerPopoverIconClass =
	"size-3.5 shrink-0 stroke-[1.5] text-muted-foreground"

export function composerPopoverItemClass(isActive: boolean, disabled = false): string {
	return cn(
		"flex w-full items-center gap-2 rounded-md px-2.5 py-1.5 text-left text-[13px] leading-5 transition-colors",
		isActive
			? "bg-muted/80 text-foreground"
			: "text-foreground hover:bg-black/[0.04] dark:hover:bg-white/[0.06]",
		disabled && "cursor-not-allowed opacity-45 hover:bg-transparent dark:hover:bg-transparent",
	)
}
