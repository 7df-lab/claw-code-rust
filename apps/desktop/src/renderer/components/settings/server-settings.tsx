/**
 * Settings tab for the local Devo stdio runtime.
 */

import { Button } from "@devo/ui/components/button"
import { Input } from "@devo/ui/components/input"
import {
	Select,
	SelectContent,
	SelectItem,
	SelectTrigger,
	SelectValue,
} from "@devo/ui/components/select"
import { ChevronDownIcon, ChevronRightIcon, RefreshCwIcon } from "lucide-react"
import { useEffect, useState } from "react"
import { useAtomValue } from "jotai"
import type { NativeTrafficLogState, NetworkProxyMode } from "../../../preload/api"
import { normalizeProxyUrl } from "../../../shared/network-proxy"
import { serverConnectedAtom, serverUrlAtom } from "../../atoms/connection"
import { useSettings } from "../../hooks/use-settings"
import { SettingsHeader } from "./settings-header"
import { SettingsRow } from "./settings-row"
import { SettingsSection } from "./settings-section"

const isElectron = typeof window !== "undefined" && "devo" in window

interface ServerSettingsProps {
	initialNativeTrafficLogState?: NativeTrafficLogState | null
}

export function ServerSettings({ initialNativeTrafficLogState = null }: ServerSettingsProps = {}) {
	const connected = useAtomValue(serverConnectedAtom)
	const url = useAtomValue(serverUrlAtom)
	const { settings, updateSettings } = useSettings()
	const [restarting, setRestarting] = useState(false)
	const [nativeTrafficLogState, setNativeTrafficLogState] = useState<NativeTrafficLogState | null>(
		initialNativeTrafficLogState,
	)
	const networkProxy = settings.servers.networkProxy
	const customProxyUrl = normalizeProxyUrl(networkProxy.proxyUrl)

	useEffect(() => {
		if (!isElectron) return
		let cancelled = false
		void window.devo.nativeTraffic
			.getState()
			.then((state) => {
				if (!cancelled) setNativeTrafficLogState(state)
			})
			.catch((error) => {
				console.error("Failed to load Native traffic log state:", error)
			})
		return () => {
			cancelled = true
		}
	}, [])

	async function restart() {
		if (!isElectron) return
		setRestarting(true)
		try {
			await window.devo.restartDevo()
		} finally {
			setRestarting(false)
		}
	}

	return (
		<div className="space-y-10">
			<SettingsHeader
				title="Server"
				description="Devo Desktop manages a private local stdio Native process."
			/>

			<SettingsSection>
				<SettingsRow
					label="Local runtime"
					description={url ?? "stdio://local"}
				>
					<div className="flex items-center gap-2 text-sm">
						<span
							className={`size-2 rounded-full ${connected ? "bg-emerald-500" : "bg-muted-foreground"}`}
						/>
						<span className="text-muted-foreground">{connected ? "Connected" : "Offline"}</span>
					</div>
				</SettingsRow>
			</SettingsSection>

			<SettingsSection
				title="Network proxy"
				description="Restart runtime to apply proxy changes."
			>
				<SettingsRow
					label="Proxy mode"
					description="Use inherited environment proxy settings, a custom proxy, or no proxy"
				>
					<Select
						value={networkProxy.mode}
						onValueChange={(value) => {
							updateSettings({
								servers: {
									networkProxy: {
										mode: value as NetworkProxyMode,
									},
								},
							})
						}}
						items={{
							system: "System",
							custom: "Custom",
							off: "Off",
						}}
					>
						<SelectTrigger className="min-w-[140px]">
							<SelectValue />
						</SelectTrigger>
						<SelectContent>
							<SelectItem value="system">System</SelectItem>
							<SelectItem value="custom">Custom</SelectItem>
							<SelectItem value="off">Off</SelectItem>
						</SelectContent>
					</Select>
				</SettingsRow>
				{networkProxy.mode === "custom" && (
					<>
						<SettingsRow
							label="Proxy URL"
							description="http, https, socks5, or socks5h"
							htmlFor="server-network-proxy-url"
						>
							<div className="flex w-72 flex-col gap-1">
								<Input
									id="server-network-proxy-url"
									value={networkProxy.proxyUrl}
									placeholder="socks5h://127.0.0.1:7890"
									onChange={(event) => {
										updateSettings({
											servers: {
												networkProxy: {
													proxyUrl: event.currentTarget.value,
												},
											},
										})
									}}
								/>
								{networkProxy.proxyUrl.trim() && !customProxyUrl && (
									<span className="text-xs text-destructive">Invalid proxy URL</span>
								)}
							</div>
						</SettingsRow>
						<SettingsRow
							label="Bypass"
							description="Comma-separated hosts that should connect directly"
							htmlFor="server-network-no-proxy"
						>
							<Input
								id="server-network-no-proxy"
								className="w-72"
								value={networkProxy.noProxy}
								onChange={(event) => {
									updateSettings({
										servers: {
											networkProxy: {
												noProxy: event.currentTarget.value,
											},
										},
									})
								}}
							/>
						</SettingsRow>
					</>
				)}
			</SettingsSection>

			<SettingsSection>
				<SettingsRow
					label="Restart runtime"
					description="Stop the current child process and start a fresh Devo stdio server"
				>
					<Button size="sm" variant="outline" onClick={restart} disabled={restarting}>
						<RefreshCwIcon
							aria-hidden="true"
							className={`size-3.5 ${restarting ? "animate-spin" : ""}`}
						/>
						Restart
					</Button>
				</SettingsRow>
			</SettingsSection>

			<NativeTrafficLogStatus state={nativeTrafficLogState} />
		</div>
	)
}

export function NativeTrafficLogStatus({
	state,
	initialExpanded = false,
}: {
	state: NativeTrafficLogState | null
	initialExpanded?: boolean
}) {
	const [expanded, setExpanded] = useState(initialExpanded)

	if (!state?.enabled) return null

	const ToggleIcon = expanded ? ChevronDownIcon : ChevronRightIcon

	return (
		<SettingsSection
			title="Developer options"
			description="Tools for inspecting the managed local runtime."
		>
			<SettingsRow
				label="Native traffic log"
				description="Protocol trace enabled via DEVO_PROTOCOL_TRACE"
			>
				<Button
					type="button"
					size="sm"
					variant="ghost"
					aria-expanded={expanded}
					onClick={() => setExpanded((value) => !value)}
				>
					<ToggleIcon aria-hidden="true" className="size-3.5" />
					{expanded ? "Hide" : "Show"}
				</Button>
			</SettingsRow>
			{expanded && (
				<div className="space-y-2 px-5 py-3.5">
					<div className="text-[15px] font-normal tracking-[-0.01em]">Current log file</div>
					{state.path ? (
						<code className="block truncate rounded-md bg-muted px-2 py-1.5 text-[13px] text-muted-foreground">
							{state.path}
						</code>
					) : (
						<p className="text-[13px] text-muted-foreground">No log file path is available.</p>
					)}
					<p className="text-[13px] leading-5 text-muted-foreground">
						Traces are written under DEVO_HOME/traces/ (default ~/.devo/traces/). The log may
						include prompts, paths, tool arguments, and provider details.
					</p>
				</div>
			)}
		</SettingsSection>
	)
}
