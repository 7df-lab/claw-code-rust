import { type ChildProcessWithoutNullStreams, spawn } from "node:child_process"
import { EventEmitter } from "node:events"
import { createInterface } from "node:readline"
import { homedir } from "node:os"
import path from "node:path"
import type { NetworkProxySettings } from "../preload/api"
import { normalizeNoProxy, normalizeProxyUrl } from "../shared/network-proxy"
import type { NativeTrafficLogRecord, NativeTrafficLogger } from "./native-traffic-log"
import { createLogger } from "./logger"

const log = createLogger("native-stdio-client")
export const DESKTOP_NETWORK_PROXY_MODE_ENV = "DEVO_DESKTOP_NETWORK_PROXY_MODE"
export const DESKTOP_NETWORK_PROXY_URL_ENV = "DEVO_DESKTOP_NETWORK_PROXY_URL"
export const DESKTOP_NETWORK_NO_PROXY_ENV = "DEVO_DESKTOP_NETWORK_NO_PROXY"

const PROXY_ENV_KEYS = [
	"HTTP_PROXY",
	"HTTPS_PROXY",
	"ALL_PROXY",
	"NO_PROXY",
	"http_proxy",
	"https_proxy",
	"all_proxy",
	"no_proxy",
]

export type JsonRpcId = number | string

type PendingRequest = {
	resolve: (value: unknown) => void
	reject: (error: Error) => void
	timer?: ReturnType<typeof setTimeout>
}

const REQUEST_TIMEOUT_MS = 10_000
/** Cold-starting the managed `devo server` process can exceed the default RPC budget. */
export const INITIALIZE_REQUEST_TIMEOUT_MS = 60_000
/** MCP admin RPCs may start a lazy server before listing tools. */
export const MCP_ADMIN_REQUEST_TIMEOUT_MS = 60_000

export function requestTimeoutMsForMethod(method: string, fallbackMs: number): number | undefined {
	if (method === "initialize") {
		return Math.max(fallbackMs, INITIALIZE_REQUEST_TIMEOUT_MS)
	}
	if (method === "provider/validate") {
		return undefined
	}
	if (method === "mcp/tools" || method === "mcp/set_enabled") {
		return Math.max(fallbackMs, MCP_ADMIN_REQUEST_TIMEOUT_MS)
	}
	return fallbackMs
}

export type NativeIncomingMessage =
	| { type: "response"; id: JsonRpcId; message: Record<string, unknown> }
	| { type: "notification"; method: string; params: unknown; message: Record<string, unknown> }
	| { type: "request"; id: JsonRpcId; method: string; params: unknown; message: Record<string, unknown> }
	| { type: "invalid"; error: string; line: string }

export type NativeTransportEvent =
	| { type: "notification"; method: string; params: unknown }
	| { type: "request"; id: JsonRpcId; method: string; params: unknown }
	| { type: "closed"; error?: string }

export type NativeTransportListener = (event: NativeTransportEvent) => void

export interface NativeTransport {
	request(method: string, params?: unknown, directory?: string): Promise<unknown>
	notify(method: string, params?: unknown, directory?: string): Promise<void>
	respond(id: JsonRpcId, result: unknown): Promise<void>
	subscribe(listener: NativeTransportListener): () => void
	connected(): boolean
	pid(): number | null
	stop(): void
}

export interface StdioNativeClientOptions {
	program?: string
	args?: string[]
	cwd?: string
	env?: NodeJS.ProcessEnv
	networkProxy?: NetworkProxySettings
	trafficLogger?: NativeTrafficLogger
	requestTimeoutMs?: number
}

export interface BuildServerProcessEnvOptions {
	baseEnv?: NodeJS.ProcessEnv
	homeDir?: string
	optionsEnv?: NodeJS.ProcessEnv
	pathSeparator?: string
	runtimeBinDir?: string
	networkProxy?: NetworkProxySettings
}

function toError(error: unknown): Error {
	return error instanceof Error ? error : new Error(String(error))
}

export function buildServerProcessEnv({
	baseEnv = process.env,
	homeDir = homedir(),
	optionsEnv = {},
	pathSeparator = process.platform === "win32" ? ";" : ":",
	runtimeBinDir,
	networkProxy,
}: BuildServerProcessEnvOptions = {}): NodeJS.ProcessEnv {
	const binDir = runtimeBinDir ?? path.join(homeDir, ".devo", "bin")
	const env: NodeJS.ProcessEnv = {
		...baseEnv,
		...optionsEnv,
	}
	applyDesktopNetworkProxyEnv(env, networkProxy)
	env.PATH = `${binDir}${pathSeparator}${optionsEnv.PATH ?? baseEnv.PATH ?? ""}`
	return env
}

function applyDesktopNetworkProxyEnv(
	env: NodeJS.ProcessEnv,
	networkProxy: NetworkProxySettings | undefined,
): void {
	if (!networkProxy) return
	env[DESKTOP_NETWORK_PROXY_MODE_ENV] = networkProxy.mode
	delete env[DESKTOP_NETWORK_PROXY_URL_ENV]
	delete env[DESKTOP_NETWORK_NO_PROXY_ENV]

	if (networkProxy.mode === "system") return

	for (const key of PROXY_ENV_KEYS) {
		delete env[key]
	}

	if (networkProxy.mode === "off") return

	const proxyUrl = normalizeProxyUrl(networkProxy.proxyUrl)
	if (!proxyUrl) return

	const noProxy = normalizeNoProxy(networkProxy.noProxy)
	env[DESKTOP_NETWORK_PROXY_URL_ENV] = proxyUrl
	env.HTTP_PROXY = proxyUrl
	env.HTTPS_PROXY = proxyUrl
	env.ALL_PROXY = proxyUrl
	env.http_proxy = proxyUrl
	env.https_proxy = proxyUrl
	env.all_proxy = proxyUrl
	if (noProxy) {
		env[DESKTOP_NETWORK_NO_PROXY_ENV] = noProxy
		env.NO_PROXY = noProxy
		env.no_proxy = noProxy
	}
}

export function routeNativeLine(line: string): NativeIncomingMessage {
	let message: Record<string, unknown>
	try {
		message = JSON.parse(line) as Record<string, unknown>
	} catch (error) {
		return { type: "invalid", error: error instanceof Error ? error.message : String(error), line }
	}

	const id = message.id
	const method = message.method

	if ((typeof id === "number" || typeof id === "string") && typeof method === "string") {
		return {
			type: "request",
			id,
			method,
			params: "params" in message ? message.params : {},
			message,
		}
	}

	if (typeof id === "number" || typeof id === "string") {
		return { type: "response", id, message }
	}

	if (typeof method === "string") {
		return {
			type: "notification",
			method,
			params: "params" in message ? message.params : {},
			message,
		}
	}

	return { type: "invalid", error: "JSON-RPC message has no id or method", line }
}

const DIRECTORY_SCOPED_METHODS = new Set([
	"session/list",
	"skill/list",
	"skill/set_enabled",
	"search/start",
	"command/exec",
])

function scopeRequestParams(method: string, params: unknown, directory?: string): unknown {
	if (!directory || !DIRECTORY_SCOPED_METHODS.has(method)) return params
	if (!params || typeof params !== "object" || Array.isArray(params)) return params
	if ("cwd" in params) return params
	return { ...params, cwd: directory }
}

export class StdioNativeClient implements NativeTransport {
	private child: ChildProcessWithoutNullStreams | null = null
	private nextId = 1
	private pending = new Map<JsonRpcId, PendingRequest>()
	private pendingMethods = new Map<JsonRpcId, string>()
	private events = new EventEmitter()
	private stopped = false
	private initializeResult: unknown | undefined
	private initializeInFlight: Promise<unknown> | null = null

	constructor(private readonly options: StdioNativeClientOptions = {}) {}

	start(): void {
		if (this.child) return

		const devoBinDir = path.join(homedir(), ".devo", "bin")
		const program = this.options.program ?? "devo"
		const runtimeBinDir = path.isAbsolute(program) ? path.dirname(program) : undefined
		const env = buildServerProcessEnv({
			optionsEnv: this.options.env,
			runtimeBinDir,
			networkProxy: this.options.networkProxy,
		})
		const args = this.options.args ?? ["server", "--transport", "stdio"]
		const cwd = this.options.cwd ?? homedir()

		log.info("Spawning Devo Native stdio server", {
			program,
			args,
			cwd,
			binDir: runtimeBinDir ?? devoBinDir,
		})
		const child = spawn(program, args, {
			cwd,
			env,
			stdio: "pipe",
		})
		this.child = child
		this.stopped = false

		const stdout = createInterface({ input: child.stdout })
		stdout.on("line", (line) => this.handleLine(line))

		const stderr = createInterface({ input: child.stderr })
		stderr.on("line", (line) => {
			if (line.trim()) log.warn(`[stderr] ${line}`)
		})

		child.stdin.on("error", (error) => {
			const reason = toError(error)
			log.warn("Devo Native stdio stdin failed", reason)
			this.close(reason)
		})

		child.on("error", (error) => {
			log.error("Devo Native stdio process failed", error)
			this.close(error)
		})

		child.on("exit", (code, signal) => {
			if (this.stopped) return
			const reason = `Devo Native stdio process exited with code ${code ?? "null"} signal ${signal ?? "null"}`
			log.warn(reason)
			this.close(new Error(reason))
		})
	}

	async request(method: string, params: unknown = {}, directory?: string): Promise<unknown> {
		if (method === "initialize") {
			return this.requestInitialize(params, directory)
		}
		return this.sendRequest(method, params, directory)
	}

	private async requestInitialize(params: unknown, directory?: string): Promise<unknown> {
		if (this.initializeResult !== undefined) {
			return this.initializeResult
		}
		if (this.initializeInFlight) {
			return this.initializeInFlight
		}
		this.initializeInFlight = this.sendRequest("initialize", params, directory)
			.then((result) => {
				this.initializeResult = result
				this.initializeInFlight = null
				return result
			})
			.catch((error) => {
				this.initializeInFlight = null
				throw error
			})
		return this.initializeInFlight
	}

	private async sendRequest(method: string, params: unknown, directory?: string): Promise<unknown> {
		this.start()
		const id = this.nextId++
		const child = this.requireChild()
		const scopedParams = scopeRequestParams(method, params, directory)
		const payload = { jsonrpc: "2.0", id, method, params: scopedParams }
		const response = new Promise<unknown>((resolve, reject) => {
			const timeoutMs = requestTimeoutMsForMethod(
				method,
				this.options.requestTimeoutMs ?? REQUEST_TIMEOUT_MS,
			)
			const timer = timeoutMs === undefined
				? undefined
				: setTimeout(() => {
						if (!this.pending.delete(id)) return
						this.pendingMethods.delete(id)
						reject(new Error(`${method} request ${id} timed out`))
					}, timeoutMs)
			this.pending.set(id, { resolve, reject, timer })
		})
		// Server may reply (and reject) before the caller awaits — attach a
		// no-op handler so Node does not log UnhandledPromiseRejectionWarning.
		// Real callers still observe the rejection via await / .catch.
		void response.catch(() => {})
		this.pendingMethods.set(id, method)
		try {
			await this.writeJson(child, payload)
			this.recordTraffic({
				direction: "desktop-to-server",
				kind: "request",
				id,
				method,
				payload,
			})
		} catch (error) {
			const reason = toError(error)
			const pending = this.pending.get(id)
			if (pending?.timer !== undefined) clearTimeout(pending.timer)
			this.pending.delete(id)
			this.pendingMethods.delete(id)
			this.close(reason)
			throw reason
		}
		return response
	}

	async notify(method: string, params: unknown = {}, directory?: string): Promise<void> {
		this.start()
		const child = this.requireChild()
		const scopedParams = scopeRequestParams(method, params, directory)
		const payload = { jsonrpc: "2.0", method, params: scopedParams }
		try {
			await this.writeJson(child, payload)
			this.recordTraffic({
				direction: "desktop-to-server",
				kind: "notification",
				method,
				payload,
			})
		} catch (error) {
			const reason = toError(error)
			this.close(reason)
			throw reason
		}
	}

	async respond(id: JsonRpcId, result: unknown): Promise<void> {
		this.start()
		const payload = { jsonrpc: "2.0", id, result }
		try {
			await this.writeJson(this.requireChild(), payload)
			this.recordTraffic({
				direction: "desktop-to-server",
				kind: "response",
				id,
				payload,
			})
		} catch (error) {
			const reason = toError(error)
			this.close(reason)
			throw reason
		}
	}

	subscribe(listener: NativeTransportListener): () => void {
		this.events.on("event", listener)
		return () => this.events.off("event", listener)
	}

	connected(): boolean {
		return this.child !== null && !this.child.killed && !this.stopped
	}

	pid(): number | null {
		return this.child?.pid ?? null
	}

	stop(): void {
		this.stopped = true
		const child = this.child
		this.child = null
		if (child && !child.killed) child.kill()
		this.close(new Error("Devo Native stdio client stopped"))
	}

	private handleLine(line: string): void {
		const routed = routeNativeLine(line)
		switch (routed.type) {
			case "response": {
				const method = this.pendingMethods.get(routed.id)
				this.pendingMethods.delete(routed.id)
				this.recordTraffic({
					direction: "server-to-desktop",
					kind: "response",
					id: routed.id,
					method,
					payload: routed.message,
				})
				const pending = this.pending.get(routed.id)
				if (!pending) return
				this.pending.delete(routed.id)
				if (pending.timer !== undefined) clearTimeout(pending.timer)
				const error = routed.message.error as { message?: string; code?: string } | undefined
				if (error) {
					const rejected = new Error(error.message ?? "Devo Native request failed") as Error & {
						code?: string
					}
					if (typeof error.code === "string") rejected.code = error.code
					pending.reject(rejected)
				} else {
					pending.resolve(routed.message.result)
				}
				break
			}
			case "notification":
				this.recordTraffic({
					direction: "server-to-desktop",
					kind: "notification",
					method: routed.method,
					payload: routed.message,
				})
				this.events.emit("event", {
					type: "notification",
					method: routed.method,
					params: routed.params,
				} satisfies NativeTransportEvent)
				break
			case "request":
				this.recordTraffic({
					direction: "server-to-desktop",
					kind: "request",
					id: routed.id,
					method: routed.method,
					payload: routed.message,
				})
				this.events.emit("event", {
					type: "request",
					id: routed.id,
					method: routed.method,
					params: routed.params,
				} satisfies NativeTransportEvent)
				break
			case "invalid":
				this.recordTraffic({
					direction: "system",
					kind: "invalid",
					payload: { error: routed.error, line: routed.line },
				})
				log.warn("Ignoring invalid Native stdio line", { error: routed.error, line: routed.line })
				break
		}
	}

	private writeJson(child: ChildProcessWithoutNullStreams, value: unknown): Promise<void> {
		const line = `${JSON.stringify(value)}\n`
		if (child.stdin.destroyed || child.stdin.writableEnded || !child.stdin.writable) {
			return Promise.reject(new Error("Devo Native stdio stdin is closed"))
		}

		return new Promise((resolve, reject) => {
			try {
				if (!child.stdin.write(line, (error) => {
					if (error) {
						reject(toError(error))
					} else {
						resolve()
					}
				})) {
					log.debug("Native stdio stdin applying backpressure")
				}
			} catch (error) {
				reject(toError(error))
			}
		})
	}

	private requireChild(): ChildProcessWithoutNullStreams {
		if (!this.child) throw new Error("Devo Native stdio process is not running")
		return this.child
	}

	private close(error: Error): void {
		this.child = null
		this.initializeResult = undefined
		this.initializeInFlight = null
		this.recordTraffic({
			direction: "system",
			kind: "closed",
			payload: { error: error.message },
		})
		for (const pending of this.pending.values()) {
			if (pending.timer !== undefined) clearTimeout(pending.timer)
			pending.reject(error)
		}
		this.pending.clear()
		this.pendingMethods.clear()
		this.events.emit("event", { type: "closed", error: error.message } satisfies NativeTransportEvent)
	}

	private recordTraffic(entry: NativeTrafficLogRecord): void {
		try {
			this.options.trafficLogger?.record(entry)
		} catch (error) {
			log.warn("Failed to record Native traffic", error)
		}
	}
}
