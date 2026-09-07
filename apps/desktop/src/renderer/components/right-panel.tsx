import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuGroup,
	DropdownMenuItem,
	DropdownMenuTrigger,
} from "@devo/ui/components/dropdown-menu"
import { cn } from "@devo/ui/lib/utils"
import { useAtom } from "jotai"
import {
	GitCommitHorizontalIcon,
	Maximize2Icon,
	Minimize2Icon,
	PanelBottomIcon,
	PlusIcon,
	XIcon,
} from "lucide-react"
import { memo, useCallback, type ReactNode } from "react"
import {
	type RightPanelTab,
	type RightPanelTabKind,
	DEFAULT_RIGHT_PANEL_CHANGES_TAB,
	reviewPanelOpenAtom,
	reviewPanelSettingsAtom,
	rightPanelTabsAtom,
} from "../atoms/ui"
import { DesktopTerminalPanel } from "./desktop-terminal-panel"
import { RightPanelIcon } from "./panel-icons"
import { ReviewPanel } from "./review/review-panel"

const TAB_KIND_META: Record<
	RightPanelTabKind,
	{ title: string; icon: (props: { className?: string }) => ReactNode }
> = {
	changes: {
		title: "Changes",
		icon: ({ className }) => (
			<GitCommitHorizontalIcon className={className} aria-hidden="true" />
		),
	},
	terminal: {
		title: "Terminal",
		icon: ({ className }) => <PanelBottomIcon className={className} aria-hidden="true" />,
	},
}

function createTab(kind: RightPanelTabKind, sameKindCount = 0): RightPanelTab {
	const meta = TAB_KIND_META[kind]
	return {
		id: `${kind}-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 6)}`,
		kind,
		title: sameKindCount === 0 ? meta.title : `${meta.title} ${sameKindCount + 1}`,
	}
}

interface RightPanelProps {
	sessionId: string
	directory: string
}

/**
 * Right session rail: browser-style tabs for Changes / Terminal,
 * with fullscreen + sidebar toggle on the tab strip.
 */
export const RightPanel = memo(function RightPanel({ sessionId, directory }: RightPanelProps) {
	const [tabsState, setTabsState] = useAtom(rightPanelTabsAtom)
	const [, setOpen] = useAtom(reviewPanelOpenAtom)
	const [settings, setSettings] = useAtom(reviewPanelSettingsAtom)

	const activeTab = tabsState.tabs.find((tab) => tab.id === tabsState.activeId) ?? tabsState.tabs[0]

	const handleClosePanel = useCallback(() => {
		setOpen(false)
		setSettings((prev) => (prev.expanded ? { ...prev, expanded: false } : prev))
	}, [setOpen, setSettings])

	const handleToggleExpanded = useCallback(
		() => setSettings((prev) => ({ ...prev, expanded: !prev.expanded })),
		[setSettings],
	)

	const handleSelectTab = useCallback(
		(id: string) => setTabsState((prev) => ({ ...prev, activeId: id })),
		[setTabsState],
	)

	const handleCloseTabClick = useCallback(
		(id: string) => {
			const isLast = tabsState.tabs.length <= 1
			setTabsState((prev) => {
				const index = prev.tabs.findIndex((tab) => tab.id === id)
				if (index < 0) return prev
				const nextTabs = prev.tabs.filter((tab) => tab.id !== id)
				if (nextTabs.length === 0) {
					// Reuse a stable Changes tab id so ReviewPanel does not remount
					// and wipe session UI (scope / file expand state).
					return {
						tabs: [DEFAULT_RIGHT_PANEL_CHANGES_TAB],
						activeId: DEFAULT_RIGHT_PANEL_CHANGES_TAB.id,
					}
				}
				let activeId = prev.activeId
				if (activeId === id) {
					const neighbor = nextTabs[Math.max(0, index - 1)] ?? nextTabs[0]
					activeId = neighbor.id
				}
				return { tabs: nextTabs, activeId }
			})
			if (isLast) handleClosePanel()
		},
		[handleClosePanel, setTabsState, tabsState.tabs.length],
	)

	const handleAddTab = useCallback(
		(kind: RightPanelTabKind) => {
			setTabsState((prev) => {
				const sameKind = prev.tabs.filter((tab) => tab.kind === kind).length
				const next = createTab(kind, sameKind)
				return {
					tabs: [...prev.tabs, next],
					activeId: next.id,
				}
			})
		},
		[setTabsState],
	)

	return (
		<div className="flex h-full min-h-0 flex-col bg-background">
			{/* Browser-style tab strip: white strip, gray tabs */}
			<div className="flex h-9 shrink-0 items-center gap-0.5 border-b border-border/60 bg-background px-1.5">
				<nav
					className="flex min-w-0 flex-1 items-center gap-0.5 overflow-x-auto"
					aria-label="Right panel tabs"
				>
					{tabsState.tabs.map((tab) => {
						const active = tab.id === activeTab?.id
						const Icon = TAB_KIND_META[tab.kind].icon
						return (
							<div
								key={tab.id}
								className={cn(
									"group relative flex h-7 max-w-[160px] min-w-[88px] items-center gap-1 rounded-md px-2 text-[12px] transition-colors",
									active
										? "bg-muted text-foreground"
										: "text-muted-foreground hover:bg-muted/60 hover:text-foreground",
								)}
							>
								<button
									type="button"
									onClick={() => handleSelectTab(tab.id)}
									className="flex min-w-0 flex-1 items-center gap-1.5 text-left"
								>
									<Icon className="size-3 shrink-0 opacity-70" />
									<span className="truncate font-medium">{tab.title}</span>
								</button>
								<button
									type="button"
									onClick={(event) => {
										event.stopPropagation()
										handleCloseTabClick(tab.id)
									}}
									className={cn(
										"rounded p-0.5 text-muted-foreground/70 opacity-0 transition-opacity hover:bg-background/80 hover:text-foreground group-hover:opacity-100",
										active && "opacity-60",
									)}
									title="Close tab"
									aria-label={`Close ${tab.title}`}
								>
									<XIcon className="size-3" />
								</button>
							</div>
						)
					})}
					<DropdownMenu>
						<DropdownMenuTrigger
							render={
								<button
									type="button"
									className="flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted/60 hover:text-foreground"
									title="New tab"
									aria-label="New tab"
								>
									<PlusIcon className="size-3.5 stroke-[1.5]" />
								</button>
							}
						/>
						<DropdownMenuContent align="start" className="min-w-36">
							<DropdownMenuGroup>
								<DropdownMenuItem className="text-xs" onClick={() => handleAddTab("changes")}>
									<GitCommitHorizontalIcon className="size-3.5" />
									Changes
								</DropdownMenuItem>
								<DropdownMenuItem className="text-xs" onClick={() => handleAddTab("terminal")}>
									<PanelBottomIcon className="size-3.5" />
									Terminal
								</DropdownMenuItem>
							</DropdownMenuGroup>
						</DropdownMenuContent>
					</DropdownMenu>
				</nav>
				<button
					type="button"
					onClick={handleToggleExpanded}
					className="flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground/80 transition-colors hover:bg-muted/60 hover:text-foreground"
					title={settings.expanded ? "Restore panel size" : "Expand to full width"}
				>
					{settings.expanded ? (
						<Minimize2Icon className="size-3.5 stroke-[1.5]" />
					) : (
						<Maximize2Icon className="size-3.5 stroke-[1.5]" />
					)}
				</button>
				<button
					type="button"
					onClick={handleClosePanel}
					className="flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground/80 transition-colors hover:bg-muted/60 hover:text-foreground"
					title="Hide side panel"
				>
					<RightPanelIcon open className="size-4" aria-hidden="true" />
				</button>
			</div>

			<div className="relative min-h-0 flex-1">
				{tabsState.tabs.map((tab) => {
					const active = tab.id === activeTab?.id
					return (
						<div
							key={tab.id}
							className={cn(
								"absolute inset-0",
								active ? "visible" : "pointer-events-none invisible",
							)}
							aria-hidden={!active}
						>
							{tab.kind === "changes" ? (
								<ReviewPanel
									key={directory || "changes"}
									sessionId={sessionId}
									directory={directory}
									embedded
								/>
							) : (
								<DesktopTerminalPanel
									open
									directory={directory || null}
									embedded
									onOpenChange={(nextOpen) => {
										if (!nextOpen) handleCloseTabClick(tab.id)
									}}
								/>
							)}
						</div>
					)
				})}
			</div>
		</div>
	)
})
