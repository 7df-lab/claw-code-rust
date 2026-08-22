/**
 * Local-only server configuration hook.
 *
 * Desktop v1 owns a single main-process Native stdio connection. Remote URL,
 * service discovery, password, and SSH server switching are intentionally disabled.
 */

import { useAtomValue, useSetAtom } from "jotai"
import { useCallback, useEffect } from "react"
import { DEFAULT_LOCAL_SERVER, DEFAULT_SERVER_SETTINGS } from "../../shared/server-config"
import { activeServerConfigAtom, activeServerIdAtom, serversAtom } from "../atoms/connection"
import { discoveryAtom } from "../atoms/discovery"
import { resetProjectPaginationAtom, sessionIdsAtom } from "../atoms/sessions"
import { appStore } from "../atoms/store"
import { isElectron } from "../services/backend"
import { disconnect } from "../services/connection-manager"
import { resetDiscoveryGuard } from "./use-discovery"

/**
 * Syncs local server settings from the main process into Jotai atoms.
 * Existing remote settings are normalized out so this build always uses stdio.
 */
export function useServerSettingsSync() {
	const setServers = useSetAtom(serversAtom)
	const setActiveServerId = useSetAtom(activeServerIdAtom)

	useEffect(() => {
		setServers(DEFAULT_SERVER_SETTINGS.servers)
		setActiveServerId(DEFAULT_SERVER_SETTINGS.activeServerId)

		if (!isElectron) return

		window.devo.getSettings().then((settings) => {
			if (
				settings.servers?.activeServerId !== DEFAULT_SERVER_SETTINGS.activeServerId ||
				settings.servers?.servers.length !== 1 ||
				settings.servers.servers[0]?.id !== DEFAULT_LOCAL_SERVER.id
			) {
				void window.devo.updateSettings({ servers: DEFAULT_SERVER_SETTINGS })
			}
		})

		return window.devo.onSettingsChanged(() => {
			setServers(DEFAULT_SERVER_SETTINGS.servers)
			setActiveServerId(DEFAULT_SERVER_SETTINGS.activeServerId)
		})
	}, [setServers, setActiveServerId])
}

/**
 * Returns the single configured local server and active server.
 */
export function useServers() {
	const servers = useAtomValue(serversAtom)
	const activeServer = useAtomValue(activeServerConfigAtom)
	return { servers, activeServer }
}

/**
 * Backward-compatible server actions. Only switching to the built-in local
 * server is allowed in this stdio-only build.
 */
export function useServerActions() {
	const noop = useCallback(async () => {}, [])

	const switchServer = useCallback(async (serverId: string) => {
		if (serverId !== DEFAULT_LOCAL_SERVER.id) return
		if (isElectron) {
			await window.devo.updateSettings({ servers: DEFAULT_SERVER_SETTINGS })
		}
		triggerServerSwitch(DEFAULT_LOCAL_SERVER.id)
	}, [])

	return {
		addServer: noop,
		updateServer: noop,
		removeServer: noop,
		switchServer,
	}
}

// ============================================================
// Internal helpers
// ============================================================

function triggerServerSwitch(newActiveServerId: string) {
	disconnect()
	resetDiscoveryGuard()

	const currentProjects = appStore.get(discoveryAtom).projects
	const knownDirs = currentProjects.flatMap((p) => [p.worktree, ...(p.sandboxes ?? [])]).filter(Boolean) as string[]
	appStore.set(resetProjectPaginationAtom, knownDirs)
	appStore.set(sessionIdsAtom, new Set<string>())
	appStore.set(activeServerIdAtom, newActiveServerId)
	appStore.set(discoveryAtom, {
		loaded: false,
		loading: false,
		error: null,
		phase: "idle",
		projects: [],
	})
}
