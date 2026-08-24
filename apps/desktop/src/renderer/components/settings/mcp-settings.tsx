/**
 * Settings tab for MCP server connections.
 * Lists runtime servers from `mcp/list`, shows tools, and toggles enablement.
 */

import { Badge } from "@devo/ui/components/badge"
import { Button } from "@devo/ui/components/button"
import { Switch } from "@devo/ui/components/switch"
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"
import { useAtomValue } from "jotai"
import { PlugIcon, PlusIcon, RefreshCwIcon } from "lucide-react"
import { useState } from "react"
import { toast } from "sonner"
import { serverConnectedAtom } from "../../atoms/connection"
import { discoveryAtom } from "../../atoms/discovery"
import { openMcpConfigFile } from "../../services/backend"
import { getBaseClient, getProjectClient } from "../../services/connection-manager"
import { SettingsHeader } from "./settings-header"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

const isElectron = typeof window !== "undefined" && "devo" in window

interface McpServerRow {
	name: string
	status: string
	toolCount: number
}

interface McpToolRow {
	name: string
	description: string
}

function mcpServerEnabled(status: string): boolean {
	return status !== "disabled"
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
	const [openingConfig, setOpeningConfig] = useState(false)

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

	const {
		data: tools = [],
		isLoading: toolsLoading,
		error: toolsError,
	} = useQuery({
		queryKey: ["mcp-tools", directory, expanded],
		queryFn: async (): Promise<McpToolRow[]> => {
			if (!client || !expanded) return []
			const result = await client.mcp.tools({ name: expanded })
			const rows = Array.isArray(result.data) ? result.data : []
			const mapped = (rows as Array<Record<string, unknown>>).map((tool) => ({
				name: String(tool.name ?? ""),
				description: String(tool.description ?? ""),
			}))
			void queryClient.invalidateQueries({ queryKey: ["mcp-servers"] })
			return mapped
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
			void queryClient.invalidateQueries({ queryKey: ["mcp-tools"] })
		},
	})

	const openMcpConfig = async () => {
		if (!isElectron) {
			toast.error("MCP config can only be edited in the desktop app")
			return
		}
		setOpeningConfig(true)
		try {
			await openMcpConfigFile()
		} catch (err) {
			toast.error("Failed to open MCP config", {
				description: err instanceof Error ? err.message : String(err),
			})
		} finally {
			setOpeningConfig(false)
		}
	}

	const visibleServers = servers.filter((server) => {
		if (!searchQuery) return true
		const haystack = `${server.name} ${server.status}`.toLowerCase()
		return haystack.includes(searchQuery.toLowerCase())
	})

	const addButton = (
		<Button
			type="button"
			size="sm"
			variant="secondary"
			className="h-8 rounded-full px-3"
			disabled={openingConfig}
			onClick={() => void openMcpConfig()}
			aria-label="Add MCP"
		>
			<PlusIcon className="size-3.5 stroke-[1.5]" />
			Add
		</Button>
	)

	return (
		<div className={embedded ? "space-y-6 px-8 py-8" : "space-y-10"}>
			{!embedded && (
				<SettingsHeader
					title="MCP"
					action={addButton}
					description={
						<>
							Connect Model Context Protocol servers. Add servers in{" "}
							<code className="rounded bg-muted px-1 py-0.5 text-[13px]">config.toml</code>, then
							enable them here.
						</>
					}
				/>
			)}

			<SettingsSection title="Servers" action={embedded ? addButton : undefined}>
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
								? "Add a server with +, then restart the session."
								: "Try a different search."
						}
					>
						<PlugIcon className="size-4 text-muted-foreground" />
					</SettingsRow>
				)}
				{visibleServers.map((server) => {
					const enabled = mcpServerEnabled(server.status)
					const expandedThis = expanded === server.name
					return (
						<div key={server.name}>
							<SettingsRow
								label={server.name}
								description={
									enabled ? `${server.status} · ${server.toolCount} tools` : server.status
								}
							>
								<div className="flex items-center gap-2">
									<Badge variant="secondary">{server.status}</Badge>
									{enabled && (
										<Button
											size="sm"
											variant="ghost"
											onClick={() =>
												setExpanded((current) => (current === server.name ? null : server.name))
											}
										>
											Tools
										</Button>
									)}
									<Switch
										checked={enabled}
										onCheckedChange={(checked) => {
											const nextEnabled = checked === true
											if (!nextEnabled) {
												setExpanded((current) => (current === server.name ? null : current))
											}
											toggle.mutate({ name: server.name, enabled: nextEnabled })
										}}
									/>
								</div>
							</SettingsRow>
							{enabled && expandedThis && (
								<div className="space-y-1 px-5 pb-3.5 text-[13px] leading-5 text-muted-foreground">
									{toolsError ? (
										<p>Failed to load tools: {String(toolsError)}</p>
									) : toolsLoading ? (
										<p className="flex items-center gap-2">
											<RefreshCwIcon className="size-3.5 animate-spin" />
											Loading tools…
										</p>
									) : tools.length === 0 ? (
										<p>
											{server.status === "failed"
												? "Server failed to start, so no tools are available."
												: "No tools advertised."}
										</p>
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
