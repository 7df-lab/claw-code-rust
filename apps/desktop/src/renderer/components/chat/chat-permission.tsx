import { Button } from "@devo/ui/components/button"
import {
	DropdownMenu,
	DropdownMenuContent,
	DropdownMenuItem,
	DropdownMenuTrigger,
} from "@devo/ui/components/dropdown-menu"
import { ChevronDownIcon, Loader2Icon, ShieldCheckIcon, ZapIcon } from "lucide-react"
import { memo, useState } from "react"
import type { Agent, PermissionRequest, PermissionResponse } from "../../lib/types"

const scopeLabels: Partial<Record<PermissionResponse, string>> = {
	once: "Allow once",
	turn: "Allow for this turn",
	session: "Allow for this session",
	pathPrefix: "Allow this path",
	host: "Allow this host",
	tool: "Allow this tool",
	commandPrefix: "Allow this command prefix",
	commandPrefixPersist: "Always allow this command prefix",
}

interface PermissionItemProps {
	agent: Agent
	permission: PermissionRequest
	onApprove?: (
		agent: Agent,
		permissionSessionId: string,
		permissionId: string,
		response?: PermissionResponse,
	) => Promise<void>
	onDeny?: (agent: Agent, permissionSessionId: string, permissionId: string) => Promise<void>
	isConnected?: boolean
	/** When true, the permission originated from a sub-agent session */
	isFromSubAgent?: boolean
}

/**
 * Single permission request card — compact style matching the task list.
 *
 * When `isFromSubAgent` is true an indicator is shown to communicate that the
 * permission was raised by a child sub-agent, not the current session directly.
 */
export const PermissionItem = memo(function PermissionItem({
	agent,
	permission,
	onApprove,
	onDeny,
	isConnected,
	isFromSubAgent,
}: PermissionItemProps) {
	const [responding, setResponding] = useState(false)

	async function handleApprove(response: PermissionResponse = "once") {
		if (!onApprove || responding) return
		setResponding(true)
		try {
			// Pass the permission's own sessionID so sub-agent permissions are
			// routed to the correct session, not the parent agent's session.
			await onApprove(agent, permission.sessionID, permission.id, response)
		} finally {
			setResponding(false)
		}
	}

	async function handleDeny() {
		if (!onDeny || responding) return
		setResponding(true)
		try {
			await onDeny(agent, permission.sessionID, permission.id)
		} finally {
			setResponding(false)
		}
	}

	const tool = permission.metadata?.tool as string | undefined
	const command = permission.metadata?.command as string | undefined
	const availableScopes: PermissionResponse[] = Array.isArray(permission.metadata?.availableScopes)
		? permission.metadata.availableScopes.filter(
				(scope: unknown): scope is PermissionResponse => typeof scope === "string" && scope in scopeLabels,
			)
		: ["once" as const]
	const primaryScope = availableScopes.includes("once") ? "once" : availableScopes[0] ?? "once"

	return (
		<div className="mb-2 rounded-xl border border-border bg-card">
			<div className="px-3 py-2">
				{isFromSubAgent && (
					<div className="mb-1.5 flex items-center gap-1 text-[11px] text-muted-foreground/70">
						<ZapIcon className="size-3 shrink-0" aria-hidden="true" />
						<span>Sub-agent requesting permission</span>
					</div>
				)}
				<div className="flex items-center gap-1.5">
					<ShieldCheckIcon className="size-3.5 shrink-0 text-muted-foreground" aria-hidden="true" />
					<span className="text-sm text-foreground">{permission.permission}</span>
				</div>
				{(tool || command) && (
					<code className="mt-1 block truncate text-xs text-muted-foreground">
						{tool && <span>{tool}</span>}
						{tool && command && <span> </span>}
						{command && <span>{command.length > 80 ? `${command.slice(0, 80)}...` : command}</span>}
					</code>
				)}
			</div>
			<div className="flex items-center justify-end gap-2 border-t border-border px-3 py-2">
				<button
					type="button"
					onClick={handleDeny}
					disabled={!isConnected || responding}
					className="text-xs text-muted-foreground hover:text-foreground disabled:opacity-50"
				>
					Deny
				</button>
				<div className="flex items-center">
					<Button
						size="sm"
						onClick={() => handleApprove(primaryScope)}
						disabled={!isConnected || responding}
						className="h-7 rounded-r-none px-2.5 text-xs"
					>
						{responding && <Loader2Icon className="size-3 animate-spin" aria-hidden="true" />}
						Allow
					</Button>
					<DropdownMenu>
						<DropdownMenuTrigger
							render={
								<Button
									size="sm"
									disabled={!isConnected || responding}
									className="h-7 rounded-l-none border-l border-primary-foreground/20 px-1"
									aria-label="More approval options"
								/>
							}
						>
							<ChevronDownIcon className="size-3" aria-hidden="true" />
						</DropdownMenuTrigger>
						<DropdownMenuContent align="end">
							{availableScopes.map((scope) => (
								<DropdownMenuItem key={scope} onClick={() => handleApprove(scope)}>
									{scopeLabels[scope]}
								</DropdownMenuItem>
							))}
						</DropdownMenuContent>
					</DropdownMenu>
				</div>
			</div>
		</div>
	)
})
