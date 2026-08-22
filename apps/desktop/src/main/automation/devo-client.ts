/**
 * Devo SDK client factory for the automation executor.
 *
 * Creates SDK clients from the Electron main process. Unlike the renderer
 * (which proxies through IPC to bypass Chromium connection limits), the main
 * process can use standard fetch directly since it runs in Node.js.
 *
 * The client is scoped to a specific project directory so that all session
 * and worktree operations target the correct Devo instance.
 */

import type { DevoClient } from "@devo-ai/sdk/v2/client"
import { createDevoClient } from "@devo-ai/sdk/v2/client"
import { createLogger } from "../logger"
import { getNativeTransport, getServerUrl } from "../devo-manager"

const log = createLogger("automation-client")

/**
 * Creates an Devo SDK client for automation use in the main process.
 *
 * @param directory  Project directory to scope the client to
 * @returns SDK client, or null if no server is running
 */
export function createAutomationClient(directory: string): DevoClient | null {
	const url = getServerUrl()
	if (!url) {
		log.warn("Cannot create automation client: no Devo server running")
		return null
	}

	log.debug("Creating automation SDK client", { url, directory })
	return createDevoClient({
		directory,
		transport: getNativeTransport(),
	})
}

/**
 * Creates an unscoped (no directory) Devo SDK client.
 * Used for global operations like subscribing to Native events.
 */
export function createBaseAutomationClient(): DevoClient | null {
	const url = getServerUrl()
	if (!url) {
		log.warn("Cannot create base automation client: no Devo server running")
		return null
	}

	return createDevoClient({ transport: getNativeTransport() })
}
