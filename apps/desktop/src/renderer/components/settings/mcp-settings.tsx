/**
 * Settings tab for MCP server connections.
 * Lists runtime servers from `mcp/list`, shows tools, and toggles enablement.
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import { Switch } from "@devo/ui/components/switch"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useAtomValue } from "jotai"
import { PlugIcon, RefreshCwIcon } from "lucide-react"
import { useState } from "react"
import { serverConnectedAtom } from "../../atoms/connection"
import { discoveryAtom } from "../../atoms/discovery"
import { getBaseClient, getProjectClient } from "../../services/connection-manager"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

interface McpServerRow {
	name: string
	status: string
	toolCount: number
}

interface McpToolRow {
	name: string
	description: string
}

function useMcpClient() {
	const connected = useAtomValue(serverConnectedAtom)
	const discovery = useAtomValue(discoveryAtom)
	const directory = discovery.projects[0]?.worktree ?? null
	const client = (directory ? getProjectClient(directory) : null) ?? getBaseClient()
	return { client, connected, directory }
}

export function McpSettings({
	embedded = false,
	searchQuery = "",
}: {
	embedded?: boolean
	searchQuery?: string
}) {
	const { client, connected, directory } = useMcpClient()
	const queryClient = useQueryClient()
	const [expanded, setExpanded] = useState<string | null>(null)

	const { data: servers = [], isLoading, error, refetch } = useQuery({
		queryKey: ["mcp-servers", directory],
		queryFn: async (): Promise<McpServerRow[]> => {
			if (!client) return []
			const result = await client.mcp.list()
			return ((result.data ?? []) as Array<Record<string, unknown>>).map((server) => ({
				name: String(server.name ?? ""),
				status: String(server.status ?? "unknown"),
				toolCount: Number(server.toolCount ?? server.tool_count ?? 0),
			}))
		},
		enabled: connected && !!client,
	})

	const { data: tools = [] } = useQuery({
		queryKey: ["mcp-tools", directory, expanded],
		queryFn: async (): Promise<McpToolRow[]> => {
			if (!client || !expanded) return []
			const result = await client.mcp.tools({ name: expanded })
			return ((result.data ?? []) as Array<Record<string, unknown>>).map((tool) => ({
				name: String(tool.name ?? ""),
				description: String(tool.description ?? ""),
			}))
		},
		enabled: connected && !!client && !!expanded,
	})

	const toggle = useMutation({
		mutationFn: async (params: { name: string; enabled: boolean }) => {
			if (!client) throw new Error("Not connected")
			await client.mcp.setEnabled(params)
		},
		onSuccess: () => {
			void queryClient.invalidateQueries({ queryKey: ["mcp-servers"] })
		},
	})

	const visibleServers = servers.filter((server) => {
		if (!searchQuery) return true
		const haystack = `${server.name} ${server.status}`.toLowerCase()
		return haystack.includes(searchQuery.toLowerCase())
	})

	return (
		<div className={embedded ? "space-y-6 px-8 py-8" : "space-y-8"}>
			{!embedded && (
			<div>
				<h2 className="text-[22px] font-medium tracking-tight">MCP</h2>
				<p className="mt-1 text-sm text-muted-foreground">
					Connect Model Context Protocol servers. Add new servers with{" "}
					<code className="rounded bg-muted px-1 py-0.5 text-xs">devo mcp add</code>, then enable
					them here.
				</p>
			</div>
			)}

			<SettingsSection title="Servers">
				{isLoading && (
					<SettingsRow label="Loading" description="Fetching MCP server status">
						<RefreshCwIcon className="size-4 animate-spin text-muted-foreground" />
					</SettingsRow>
				)}
				{error && (
					<SettingsRow label="Failed to load servers" description={String(error)}>
						<Button size="sm" variant="outline" onClick={() => void refetch()}>
							Retry
						</Button>
					</SettingsRow>
				)}
				{!isLoading && !error && visibleServers.length === 0 && (
					<SettingsRow
						label={servers.length === 0 ? "No MCP servers" : "No matching servers"}
						description={
							servers.length === 0
								? "Add a server with the CLI, then restart the session."
								: "Try a different search."
						}
					>
						<PlugIcon className="size-4 text-muted-foreground" />
					</SettingsRow>
				)}
				{visibleServers.map((server) => {
					const enabled = server.status !== "disabled" && server.status !== "disconnected"
					return (
						<div key={server.name}>
							<SettingsRow
								label={server.name}
								description={`${server.status} · ${server.toolCount} tools`}
							>
								<div className="flex items-center gap-2">
									<Badge variant="secondary">{server.status}</Badge>
									<Button
										size="sm"
										variant="ghost"
										onClick={() => setExpanded((current) => (current === server.name ? null : server.name))}
									>
										Tools
									</Button>
									<Switch
										checked={enabled}
										onCheckedChange={(checked) =>
											toggle.mutate({ name: server.name, enabled: checked === true })
										}
									/>
								</div>
							</SettingsRow>
							{expanded === server.name && (
								<div className="space-y-1 px-4 pb-3 text-sm text-muted-foreground">
									{tools.length === 0 ? (
										<p>No tools reported yet.</p>
									) : (
										tools.map((tool) => (
											<div key={tool.name}>
												<span className="font-medium text-foreground">{tool.name}</span>
												{tool.description ? ` — ${tool.description}` : ""}
											</div>
										))
									)}
								</div>
							)}
						</div>
					)
				})}
			</SettingsSection>
		</div>
	)
}
