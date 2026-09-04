/**
 * In-pane Customize gallery: search + pill tabs + content panel.
 * Shown in the main content area without replacing the sidebar or route.
 */

import { Button } from "@devo/ui/components/button"
import { Input } from "@devo/ui/components/input"
import { cn } from "@devo/ui/lib/utils"
import { PlusIcon, SearchIcon } from "lucide-react"
import { useState } from "react"
import { McpSettings } from "../settings/mcp-settings"
import { RuleSettings } from "../settings/rule-settings"
import { SkillSettings } from "../settings/skill-settings"

export type CustomizeTab =
	| "plugins"
	| "mcps"
	| "skills"
	| "subagents"
	| "rules"
	| "commands"
	| "hooks"

const TABS: { id: CustomizeTab; label: string }[] = [
	{ id: "plugins", label: "Plugins" },
	{ id: "mcps", label: "MCPs" },
	{ id: "skills", label: "Skills" },
	{ id: "subagents", label: "Subagents" },
	{ id: "rules", label: "Rules" },
	{ id: "commands", label: "Commands" },
	{ id: "hooks", label: "Hooks" },
]

const EMPTY_COPY: Record<CustomizeTab, { title: string; description: string }> = {
	plugins: {
		title: "Extend Devo with Plugins",
		description:
			"Plugins bundle rules, skills, subagents, commands, MCP servers, and hooks into one installable package.",
	},
	mcps: {
		title: "Connect MCP servers",
		description: "Model Context Protocol servers expose tools the agent can call during a session.",
	},
	skills: {
		title: "Add Skills",
		description: "Skills teach the agent a specific workflow. Insert one in chat with /skills or @.",
	},
	subagents: {
		title: "Create Subagents",
		description: "Subagents handle a scoped task with their own tools and instructions.",
	},
	rules: {
		title: "Add Rules",
		description:
			"Agent instructions live in AGENTS.md. Devo also reads AGENTS.override.md, CLAUDE.md, and PROMPT.md.",
	},
	commands: {
		title: "Add Commands",
		description: "Commands are reusable prompts you can run from the chat input.",
	},
	hooks: {
		title: "Add Hooks",
		description: "Hooks run before or after agent turns to enforce project conventions.",
	},
}

export function CustomizeView() {
	const [query, setQuery] = useState("")
	const [tab, setTab] = useState<CustomizeTab>("plugins")
	const normalizedQuery = query.trim().toLowerCase()

	return (
		<div className="flex h-full flex-col overflow-hidden bg-background">
			<div className="mx-auto flex h-full w-full max-w-5xl flex-col px-8 py-8 sm:px-10">
				<label className="relative block">
					<span className="sr-only">Search Plugins, Skills, MCPs</span>
					<SearchIcon
						aria-hidden="true"
						className="pointer-events-none absolute top-1/2 left-3.5 size-3.5 -translate-y-1/2 stroke-[1.5] text-muted-foreground"
					/>
					<Input
						value={query}
						onChange={(event) => setQuery(event.target.value)}
						placeholder="Search Plugins, Skills, MCPs..."
						className="h-9 rounded-lg border-border/50 bg-muted/40 pr-3 pl-9 shadow-none md:text-sm"
					/>
				</label>

				<div className="mt-4 flex flex-wrap gap-1" role="tablist" aria-label="Customize">
					{TABS.map((item) => (
						<button
							key={item.id}
							type="button"
							role="tab"
							aria-selected={tab === item.id}
							onClick={() => setTab(item.id)}
							className={cn(
								"h-7 rounded-md px-2.5 text-[13px] transition-colors",
								tab === item.id
									? "bg-muted font-medium text-foreground"
									: "bg-transparent text-muted-foreground hover:bg-muted/60 hover:text-foreground",
							)}
						>
							{item.label}
						</button>
					))}
				</div>

				{/* Flat content — no nested panel framing around SettingsSection cards */}
				<div className="mt-6 min-h-0 flex-1 overflow-y-auto">
					<CustomizePanel tab={tab} searchQuery={normalizedQuery} />
				</div>
			</div>
		</div>
	)
}

function CustomizePanel({ tab, searchQuery }: { tab: CustomizeTab; searchQuery: string }) {
	if (tab === "mcps") {
		return <McpSettings embedded searchQuery={searchQuery} />
	}
	if (tab === "skills") {
		return <SkillSettings embedded searchQuery={searchQuery} />
	}
	if (tab === "rules") {
		return <RuleSettings embedded searchQuery={searchQuery} />
	}

	const copy = EMPTY_COPY[tab]
	return (
		<div className="flex min-h-full flex-col items-center justify-center px-6 py-16 text-center">
			<h2 className="text-lg font-medium tracking-tight">{copy.title}</h2>
			<p className="mt-1.5 max-w-md text-sm leading-6 text-muted-foreground">{copy.description}</p>
			<div className="mt-6">
				<Button type="button" variant="secondary" size="sm" disabled>
					<PlusIcon className="size-3.5 stroke-[1.5]" />
					Add
				</Button>
			</div>
		</div>
	)
}
