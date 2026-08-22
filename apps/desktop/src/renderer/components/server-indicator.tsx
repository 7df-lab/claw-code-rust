/**
 * Compact local runtime indicator for the sidebar footer.
 */

import { Popover, PopoverContent, PopoverTrigger } from "@devo/ui/components/popover"
import { SidebarMenu, SidebarMenuButton, SidebarMenuItem } from "@devo/ui/components/sidebar"
import { useNavigate } from "@tanstack/react-router"
import { useAtomValue } from "jotai"
import { MonitorIcon, SettingsIcon, TerminalIcon } from "lucide-react"
import { useCallback, useState } from "react"
import { activeServerConfigAtom, serverConnectedAtom } from "../atoms/connection"

export function ServerIndicator() {
	const activeServer = useAtomValue(activeServerConfigAtom)
	const connected = useAtomValue(serverConnectedAtom)
	const navigate = useNavigate()
	const [open, setOpen] = useState(false)

	const handleSettings = useCallback(() => {
		setOpen(false)
		navigate({ to: "/settings/servers" })
	}, [navigate])

	return (
		<Popover open={open} onOpenChange={setOpen}>
			<SidebarMenu className="gap-1">
				<SidebarMenuItem>
					<PopoverTrigger
						render={
							<SidebarMenuButton
								tooltip={connected ? "Devo runtime connected" : "Devo runtime offline"}
								className={
									connected
										? "h-8 gap-2.5 rounded-lg px-1.5 py-0 text-sm font-normal text-muted-foreground hover:bg-black/[0.04] active:bg-black/[0.04] dark:hover:bg-white/[0.06] dark:active:bg-white/[0.06]"
										: "h-8 gap-2.5 rounded-lg px-1.5 py-0 text-sm font-normal text-red-500 hover:bg-black/[0.04] active:bg-black/[0.04] dark:hover:bg-white/[0.06] dark:active:bg-white/[0.06]"
								}
							/>
						}
					>
						<div className="relative">
							<MonitorIcon aria-hidden="true" className="size-[18px]" />
							<span
								className={`absolute -right-0.5 -bottom-0.5 size-2 rounded-full border border-sidebar-background ${
									connected ? "bg-green-500" : "bg-red-500"
								}`}
							/>
						</div>
						<span className="truncate">{activeServer.name}</span>
						{!connected && <span className="text-[10px] text-red-500/70">(offline)</span>}
					</PopoverTrigger>
				</SidebarMenuItem>
			</SidebarMenu>

			<PopoverContent side="top" align="start" className="w-64 p-1">
				<div className="px-2 py-1.5">
					<p className="text-xs font-medium text-muted-foreground">Runtime</p>
				</div>
				<div className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-sm">
					<TerminalIcon aria-hidden="true" className="size-3.5 shrink-0 text-muted-foreground" />
					<span className="min-w-0 flex-1 truncate">Native stdio</span>
					<span
						className={`size-1.5 shrink-0 rounded-full ${connected ? "bg-green-500" : "bg-red-500"}`}
					/>
				</div>
				<div className="my-1 border-t border-border" />
				<button
					type="button"
					onClick={handleSettings}
					className="flex w-full items-center gap-2 rounded-sm px-2 py-1.5 text-left text-sm text-muted-foreground transition-colors hover:bg-accent/50 hover:text-foreground"
				>
					<SettingsIcon aria-hidden="true" className="size-3.5" />
					Runtime Settings...
				</button>
			</PopoverContent>
		</Popover>
	)
}
