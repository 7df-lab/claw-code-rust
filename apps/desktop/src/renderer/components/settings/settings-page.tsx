import { SidebarContent } from "@devo/ui/components/sidebar"
import { Outlet, useNavigate, useRouterState } from "@tanstack/react-router"
import { useAtomValue } from "jotai"
import {
	ArrowLeftIcon,
	BellIcon,
	BookOpenIcon,
	BoxIcon,
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
import { TopActionRow, sidebarPrimaryIconClass } from "../sidebar/sidebar-top-action"

// ============================================================
// Tab definitions
// ============================================================

type SettingsTab =
	| "general"
	| "servers"
	| "providers"
	| "models"
	| "mcp"
	| "skills"
	| "notifications"
	| "worktrees"
	| "setup"
	| "about"

const tabs: { id: SettingsTab; label: string; icon: typeof SettingsIcon }[] = [
	{ id: "general", label: "General", icon: SettingsIcon },
	{ id: "servers", label: "Servers", icon: ServerIcon },
	{ id: "providers", label: "Providers", icon: PlugIcon },
	{ id: "models", label: "Models", icon: BoxIcon },
	{ id: "mcp", label: "MCP", icon: PlugIcon },
	{ id: "skills", label: "Skills", icon: BookOpenIcon },
	{ id: "notifications", label: "Notifications", icon: BellIcon },
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
			<div className="mx-auto max-w-3xl px-8 py-10 sm:px-10">
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
			<div className="flex min-h-0 flex-1 flex-col gap-1 overflow-auto px-3 pb-7">
				<TopActionRow
					icon={<ArrowLeftIcon aria-hidden="true" className={sidebarPrimaryIconClass} />}
					onClick={() => navigate(resolveSettingsBackTarget(lastAppRoute))}
				>
					Back to app
				</TopActionRow>
				{tabs.map((tab) => {
					const Icon = tab.icon
					return (
						<TopActionRow
							key={tab.id}
							icon={<Icon aria-hidden="true" className={sidebarPrimaryIconClass} />}
							onClick={() => navigate({ to: `/settings/${tab.id}` })}
							isActive={activeTab === tab.id}
						>
							{tab.label}
						</TopActionRow>
					)
				})}
			</div>
		</SidebarContent>
	)
}
