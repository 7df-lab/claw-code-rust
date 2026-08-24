import {
	SidebarContent,
	SidebarMenu,
	SidebarMenuButton,
	SidebarMenuItem,
} from "@devo/ui/components/sidebar"
import { Outlet, useNavigate, useRouterState } from "@tanstack/react-router"
import { useAtomValue } from "jotai"
import {
	ArrowLeftIcon,
	BellIcon,
	BookOpenIcon,
	GitForkIcon,
	InfoIcon,
	PlugIcon,
	ServerIcon,
	SettingsIcon,
	WrenchIcon,
} from "lucide-react"
import { useEffect } from "react"
import { lastAppRouteAtom } from "../../atoms/ui"
import { resolveSettingsBackTarget } from "../../lib/app-navigation"
import { useSetSidebarSlot } from "../sidebar-slot-context"

// ============================================================
// Tab definitions
// ============================================================

type SettingsTab =
	| "general"
	| "servers"
	| "mcp"
	| "skills"
	| "notifications"
	| "providers"
	| "worktrees"
	| "setup"
	| "about"

const tabs: { id: SettingsTab; label: string; icon: typeof SettingsIcon }[] = [
	{ id: "general", label: "General", icon: SettingsIcon },
	{ id: "servers", label: "Servers", icon: ServerIcon },
	{ id: "mcp", label: "MCP", icon: PlugIcon },
	{ id: "skills", label: "Skills", icon: BookOpenIcon },
	{ id: "notifications", label: "Notifications", icon: BellIcon },
	{ id: "providers", label: "Providers", icon: PlugIcon },
	{ id: "worktrees", label: "Worktrees", icon: GitForkIcon },
	{ id: "setup", label: "Setup", icon: WrenchIcon },
	{ id: "about", label: "About", icon: InfoIcon },
]

// ============================================================
// Settings layout (renders <Outlet /> for child routes)
// ============================================================

export function SettingsPage() {
	const { setContent, setFooter } = useSetSidebarSlot()

	useEffect(() => {
		setContent(<SettingsSidebarContent />)
		setFooter(false)
		return () => {
			setContent(null)
			setFooter(null)
		}
	}, [setContent, setFooter])

	return (
		<div className="h-full overflow-y-auto">
			<div className="mx-auto max-w-2xl px-10 py-10">
				<Outlet />
			</div>
		</div>
	)
}

// ============================================================
// Sidebar content injected via slot context
// ============================================================

function SettingsSidebarContent() {
	const navigate = useNavigate()
	const pathname = useRouterState({ select: (s) => s.location.pathname })
	const lastAppRoute = useAtomValue(lastAppRouteAtom)

	// Derive active tab from the last path segment (e.g. "/settings/general" -> "general")
	const activeTab = pathname.split("/").pop() || "general"

	return (
		<SidebarContent className="gap-0 bg-transparent px-0 pb-3">
			<div className="flex shrink-0 flex-col gap-1 px-3 pb-7">
				<button
					type="button"
					onClick={() => navigate(resolveSettingsBackTarget(lastAppRoute))}
					className="flex h-8 w-full items-center gap-2.5 rounded-lg px-1.5 text-left text-[13px] font-normal text-muted-foreground transition-colors hover:bg-black/[0.04] hover:text-sidebar-foreground dark:hover:bg-white/[0.06]"
				>
					<span className="flex size-[18px] shrink-0 items-center justify-center text-sidebar-foreground/90">
						<ArrowLeftIcon aria-hidden="true" className="size-[18px]" />
					</span>
					<span className="min-w-0 flex-1 truncate">Back to app</span>
				</button>
			</div>
			<div className="min-h-0 flex-1 overflow-auto px-3 pb-2">
				<SidebarMenu>
					{tabs.map((tab) => {
						const Icon = tab.icon
						return (
							<SidebarMenuItem key={tab.id}>
								<SidebarMenuButton
									isActive={activeTab === tab.id}
									onClick={() => navigate({ to: `/settings/${tab.id}` })}
									tooltip={tab.label}
								>
									<Icon aria-hidden="true" className="size-4" />
									<span>{tab.label}</span>
								</SidebarMenuButton>
							</SidebarMenuItem>
						)
					})}
				</SidebarMenu>
			</div>
		</SidebarContent>
	)
}
