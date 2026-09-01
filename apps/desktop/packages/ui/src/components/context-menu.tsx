"use client"

import { ContextMenu as ContextMenuPrimitive } from "@base-ui/react/context-menu"
import { cn } from "@devo/ui/lib/utils"
import { CheckIcon, ChevronRightIcon } from "lucide-react"
import type * as React from "react"
import {
	optionMenuContentClass,
	optionMenuItemClass,
	optionMenuSeparatorClass,
} from "./option-menu-styles"

function ContextMenu({ ...props }: ContextMenuPrimitive.Root.Props) {
	return <ContextMenuPrimitive.Root data-slot="context-menu" {...props} />
}

function ContextMenuPortal({ ...props }: ContextMenuPrimitive.Portal.Props) {
	return <ContextMenuPrimitive.Portal data-slot="context-menu-portal" {...props} />
}

function ContextMenuTrigger({ className, ...props }: ContextMenuPrimitive.Trigger.Props) {
	return (
		<ContextMenuPrimitive.Trigger
			data-slot="context-menu-trigger"
			className={cn("select-none", className)}
			{...props}
		/>
	)
}

function ContextMenuContent({
	className,
	align = "start",
	alignOffset = 4,
	side = "right",
	sideOffset = 0,
	...props
}: ContextMenuPrimitive.Popup.Props &
	Pick<ContextMenuPrimitive.Positioner.Props, "align" | "alignOffset" | "side" | "sideOffset">) {
	return (
		<ContextMenuPrimitive.Portal>
			<ContextMenuPrimitive.Positioner
				className="isolate z-50 outline-none"
				align={align}
				alignOffset={alignOffset}
				side={side}
				sideOffset={sideOffset}
			>
				<ContextMenuPrimitive.Popup
					data-slot="context-menu-content"
					className={cn(
						"data-open:animate-in data-closed:animate-out data-closed:fade-out-0 data-open:fade-in-0 data-closed:zoom-out-95 data-open:zoom-in-95 data-[side=bottom]:slide-in-from-top-2 data-[side=left]:slide-in-from-right-2 data-[side=right]:slide-in-from-left-2 data-[side=top]:slide-in-from-bottom-2 text-popover-foreground min-w-36 duration-100 data-[side=inline-start]:slide-in-from-right-2 data-[side=inline-end]:slide-in-from-left-2 z-50 max-h-(--available-height) origin-(--transform-origin) overflow-x-hidden overflow-y-auto outline-none",
						optionMenuContentClass,
						className,
					)}
					{...props}
				/>
			</ContextMenuPrimitive.Positioner>
		</ContextMenuPrimitive.Portal>
	)
}

function ContextMenuGroup({ ...props }: ContextMenuPrimitive.Group.Props) {
	return <ContextMenuPrimitive.Group data-slot="context-menu-group" {...props} />
}

function ContextMenuLabel({
	className,
	inset,
	...props
}: ContextMenuPrimitive.GroupLabel.Props & {
	inset?: boolean
}) {
	return (
		<ContextMenuPrimitive.GroupLabel
			data-slot="context-menu-label"
			data-inset={inset}
			className={cn(
				"text-muted-foreground px-2 py-1.5 text-xs font-medium data-inset:pl-8",
				className,
			)}
			{...props}
		/>
	)
}

function ContextMenuItem({
	className,
	inset,
	variant = "default",
	...props
}: ContextMenuPrimitive.Item.Props & {
	inset?: boolean
	variant?: "default" | "destructive"
}) {
	return (
		<ContextMenuPrimitive.Item
			data-slot="context-menu-item"
			data-inset={inset}
			data-variant={variant}
			className={cn(
				"focus:bg-muted focus:text-foreground data-[variant=destructive]:text-destructive data-[variant=destructive]:focus:bg-destructive/10 dark:data-[variant=destructive]:focus:bg-destructive/20 data-[variant=destructive]:focus:text-destructive data-[variant=destructive]:*:[svg]:text-destructive focus:*:[svg]:text-muted-foreground data-inset:pl-8 [&_svg:not([class*='size-'])]:size-4 group/context-menu-item relative flex cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
				optionMenuItemClass,
				className,
			)}
			{...props}
		/>
	)
}

function ContextMenuSub({ ...props }: ContextMenuPrimitive.SubmenuRoot.Props) {
	return <ContextMenuPrimitive.SubmenuRoot data-slot="context-menu-sub" {...props} />
}

function ContextMenuSubTrigger({
	className,
	inset,
	children,
	...props
}: ContextMenuPrimitive.SubmenuTrigger.Props & {
	inset?: boolean
}) {
	return (
		<ContextMenuPrimitive.SubmenuTrigger
			data-slot="context-menu-sub-trigger"
			data-inset={inset}
			className={cn(
				"focus:bg-accent focus:text-accent-foreground data-open:bg-accent data-open:text-accent-foreground data-inset:pl-8 [&_svg:not([class*='size-'])]:size-4 flex cursor-default items-center outline-hidden select-none [&_svg]:pointer-events-none [&_svg]:shrink-0",
				optionMenuItemClass,
				className,
			)}
			{...props}
		>
			{children}
			<ChevronRightIcon className="ml-auto" />
		</ContextMenuPrimitive.SubmenuTrigger>
	)
}

function ContextMenuSubContent({
	className,
	...props
}: React.ComponentProps<typeof ContextMenuContent>) {
	return (
		<ContextMenuContent
			data-slot="context-menu-sub-content"
			className={className}
			side="right"
			{...props}
		/>
	)
}

function ContextMenuCheckboxItem({
	className,
	children,
	checked,
	inset,
	...props
}: ContextMenuPrimitive.CheckboxItem.Props & {
	inset?: boolean
}) {
	return (
		<ContextMenuPrimitive.CheckboxItem
			data-slot="context-menu-checkbox-item"
			data-inset={inset}
			className={cn(
				"focus:bg-accent focus:text-accent-foreground pr-9 data-inset:pl-8 [&_svg:not([class*='size-'])]:size-4 relative flex cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
				optionMenuItemClass,
				className,
			)}
			checked={checked}
			{...props}
		>
			<span className="absolute right-2 pointer-events-none">
				<ContextMenuPrimitive.CheckboxItemIndicator>
					<CheckIcon />
				</ContextMenuPrimitive.CheckboxItemIndicator>
			</span>
			{children}
		</ContextMenuPrimitive.CheckboxItem>
	)
}

function ContextMenuRadioGroup({ ...props }: ContextMenuPrimitive.RadioGroup.Props) {
	return <ContextMenuPrimitive.RadioGroup data-slot="context-menu-radio-group" {...props} />
}

function ContextMenuRadioItem({
	className,
	children,
	inset,
	...props
}: ContextMenuPrimitive.RadioItem.Props & {
	inset?: boolean
}) {
	return (
		<ContextMenuPrimitive.RadioItem
			data-slot="context-menu-radio-item"
			data-inset={inset}
			className={cn(
				"focus:bg-accent focus:text-accent-foreground pr-9 data-inset:pl-8 [&_svg:not([class*='size-'])]:size-4 relative flex cursor-default items-center outline-hidden select-none data-disabled:pointer-events-none data-disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
				optionMenuItemClass,
				className,
			)}
			{...props}
		>
			<span className="absolute right-2 pointer-events-none">
				<ContextMenuPrimitive.RadioItemIndicator>
					<CheckIcon />
				</ContextMenuPrimitive.RadioItemIndicator>
			</span>
			{children}
		</ContextMenuPrimitive.RadioItem>
	)
}

function ContextMenuSeparator({ className, ...props }: ContextMenuPrimitive.Separator.Props) {
	return (
		<ContextMenuPrimitive.Separator
			data-slot="context-menu-separator"
			className={cn("bg-border -mx-1 h-px", optionMenuSeparatorClass, className)}
			{...props}
		/>
	)
}

function ContextMenuShortcut({ className, ...props }: React.ComponentProps<"span">) {
	return (
		<span
			data-slot="context-menu-shortcut"
			className={cn(
				"text-muted-foreground group-focus/context-menu-item:text-accent-foreground ml-auto text-xs tracking-widest",
				className,
			)}
			{...props}
		/>
	)
}

export {
	ContextMenu,
	ContextMenuTrigger,
	ContextMenuContent,
	ContextMenuItem,
	ContextMenuCheckboxItem,
	ContextMenuRadioItem,
	ContextMenuLabel,
	ContextMenuSeparator,
	ContextMenuShortcut,
	ContextMenuGroup,
	ContextMenuPortal,
	ContextMenuSub,
	ContextMenuSubContent,
	ContextMenuSubTrigger,
	ContextMenuRadioGroup,
}
