import { DESKTOP_INITIALIZE_PARAMS } from "@devo-ai/sdk/v2/client"
import type { JsonRpcId, NativeTransport, NativeTransportEvent, NativeTransportListener } from "./native-stdio-client"
import { execFile } from "node:child_process"
import { promisify } from "node:util"
import { app } from "electron"
import {
	DEVO_HOME_ENV,
	PROTOCOL_TRACE_ENV,
	PROTOCOL_TRACE_FILE_ENV,
	createNativeTrafficLoggerFromEnv,
	type NativeTrafficLogger,
	type NativeTrafficLogState,
} from "./native-traffic-log"
import { StdioNativeClient } from "./native-stdio-client"
import { resolveDevoProgram } from "./devo-program"
import { createLogger } from "./logger"
import { startNotificationWatcher, stopNotificationWatcher } from "./notification-watcher"
import { getSettings } from "./settings-store"
import { waitForEnv } from "./shell-env"

const log = createLogger("devo-manager")

const execFileAsync = promisify(execFile)

const STDIO_URL = "stdio://local"
const nativeTrafficLogStartupEnv = {
	[DEVO_HOME_ENV]: process.env[DEVO_HOME_ENV],
	[PROTOCOL_TRACE_ENV]: process.env[PROTOCOL_TRACE_ENV],
	[PROTOCOL_TRACE_FILE_ENV]: process.env[PROTOCOL_TRACE_FILE_ENV],
}

export interface DevoServer {
	url: string
	transport: "stdio"
	pid: number | null
	managed: boolean
}

let stdioClient: StdioNativeClient | null = null
let server: DevoServer | null = null
let initializing: Promise<DevoServer> | null = null
let nativeTrafficLogger: NativeTrafficLogger | null = null
const serverReadyListeners = new Set<() => void>()

export async function ensureServer(): Promise<DevoServer> {
	if (server && stdioClient?.connected()) return server
	if (initializing) return initializing

	initializing = startServer().finally(() => {
		initializing = null
	})
	return initializing
}

export function getServerUrl(): string | null {
	return server?.url ?? null
}

export function onServerReady(listener: () => void): () => void {
	serverReadyListeners.add(listener)
	if (server && stdioClient?.connected()) {
		queueMicrotask(() => {
			if (serverReadyListeners.has(listener) && server && stdioClient?.connected()) {
				listener()
			}
		})
	}
	return () => {
		serverReadyListeners.delete(listener)
	}
}

export function stopServer(): boolean {
	stopNotificationWatcher()
	const hadClient = stdioClient !== null
	stdioClient?.stop()
	stdioClient = null
	server = null
	return hadClient
}

export async function restartServer(): Promise<DevoServer> {
	stopServer()
	return ensureServer()
}

/**
 * Minimum spacing between protocol-mismatch recycles so a genuine schema bug
 * in the current build cannot restart-loop the app.
 */
const PROTOCOL_RECYCLE_COOLDOWN_MS = 5 * 60_000
let lastProtocolRecycleAt = 0

/**
 * Recovers from a stale singleton server.
 *
 * At most one real devo-server runs per DEVO_HOME (`~/.devo/server.lock`); a
 * spawned stdio child that loses the lock silently proxies to the holder
 * (server `singleton.rs`). When the holder is an older build, this build's
 * protocol schema rejects its traffic (ProtocolValidationError on e.g.
 * subscription/create replay). Ask the singleton to shut down, then restart
 * our own child, which becomes the real server.
 *
 * Returns false when the cooldown suppressed the recycle.
 */
export async function recycleServerForProtocolMismatch(reason: string): Promise<boolean> {
	const now = Date.now()
	if (now - lastProtocolRecycleAt < PROTOCOL_RECYCLE_COOLDOWN_MS) {
		log.warn(
			"Skipping Devo server recycle (cooldown); if the error persists, restart the app or run 'devo server --shutdown'",
			{ reason },
		)
		return false
	}
	lastProtocolRecycleAt = now
	log.warn("Recycling Devo server after protocol mismatch", { reason })

	const program = resolveDevoProgram({
		appPath: app.getAppPath(),
		env: process.env,
		isPackaged: app.isPackaged,
		resourcesPath: process.resourcesPath,
	})
	try {
		await execFileAsync(program, ["server", "--shutdown"], {
			timeout: 15_000,
			windowsHide: true,
		})
	} catch (error) {
		// No singleton is running (or it died mid-handshake): the restart below
		// still brings up a fresh real server of this build.
		log.debug("Singleton shutdown call did not succeed", { error: String(error) })
	}

	await restartServer()
	return true
}

export async function requestNative(
	method: string,
	params?: unknown,
	directory?: string,
): Promise<unknown> {
	const client = await ensureClient()
	return client.request(method, params, directory)
}

export async function notifyNative(
	method: string,
	params?: unknown,
	directory?: string,
): Promise<void> {
	const client = await ensureClient()
	return client.notify(method, params, directory)
}

export async function respondNative(id: JsonRpcId, result: unknown): Promise<void> {
	const client = await ensureClient()
	await client.respond(id, result)
}

export function subscribeNative(listener: NativeTransportListener): () => void {
	const client = getOrCreateClient()
	return client.subscribe(listener)
}

export function isNativeConnected(): boolean {
	return stdioClient?.connected() ?? false
}

const sharedNativeTransport: NativeTransport = {
	request: requestNative,
	notify: notifyNative,
	respond: respondNative,
	subscribe: subscribeNative,
	connected: isNativeConnected,
	pid: () => stdioClient?.pid() ?? null,
	stop: stopServer,
}

export function getNativeTransport(): NativeTransport {
	return sharedNativeTransport
}

export function getNativeTrafficLogState(): NativeTrafficLogState {
	return getNativeTrafficLogger().getState()
}

async function startServer(): Promise<DevoServer> {
	await waitForEnv()
	const client = getOrCreateClient()
	client.start()

	await initialize(client)

	server = {
		url: STDIO_URL,
		transport: "stdio",
		pid: client.pid(),
		managed: true,
	}
	startNotificationWatcher(getNativeTransport())
	notifyServerReady()
	log.info("Devo Native stdio server ready", { pid: server.pid })
	return server
}

async function ensureClient(): Promise<StdioNativeClient> {
	await ensureServer()
	return getOrCreateClient()
}

function getOrCreateClient(): StdioNativeClient {
	if (!stdioClient) {
		const program = resolveDevoProgram({
			appPath: app.getAppPath(),
			env: process.env,
			isPackaged: app.isPackaged,
			resourcesPath: process.resourcesPath,
		})
			stdioClient = new StdioNativeClient({
				program,
				networkProxy: getSettings().servers.networkProxy,
				trafficLogger: getNativeTrafficLogger(),
			})
		stdioClient.subscribe(handleTransportEvent)
	}
	return stdioClient
}

function getNativeTrafficLogger(): NativeTrafficLogger {
	if (!nativeTrafficLogger) {
		nativeTrafficLogger = createNativeTrafficLoggerFromEnv({
			env: nativeTrafficLogStartupEnv,
		})
	}
	return nativeTrafficLogger
}

function handleTransportEvent(event: NativeTransportEvent): void {
	if (event.type === "closed") {
		log.warn("Devo Native stdio transport closed", { error: event.error })
		server = null
	}
}

function notifyServerReady(): void {
	for (const listener of serverReadyListeners) {
		try {
			listener()
		} catch (error) {
			log.warn("Server-ready listener failed", error)
		}
	}
}

async function initialize(client: StdioNativeClient): Promise<void> {
	try {
		await client.request("initialize", DESKTOP_INITIALIZE_PARAMS)
	} catch (error) {
		// A timed-out initialize can still leave a warming stdio child alive; retry once
		// before surfacing the error to the renderer (common when opening a session cold).
		if (client.connected() && isInitializeTimeoutError(error)) {
			log.warn("initialize timed out during cold start; retrying once")
			await client.request("initialize", DESKTOP_INITIALIZE_PARAMS)
			return
		}
		throw error
	}
}

function isInitializeTimeoutError(error: unknown): boolean {
	return error instanceof Error && error.message.startsWith("initialize request ")
}
