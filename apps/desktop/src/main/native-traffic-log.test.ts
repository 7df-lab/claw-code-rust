import { describe, expect, test } from "bun:test"
import { existsSync, mkdtempSync, readFileSync, rmSync } from "node:fs"
import { tmpdir } from "node:os"
import path from "node:path"
import {
	DEVO_HOME_ENV,
	PROTOCOL_TRACE_ENV,
	PROTOCOL_TRACE_FILE_ENV,
	createNativeTrafficLoggerFromEnv,
	findDevoHome,
	formatProtocolTraceTimestamp,
	isProtocolTraceEnabled,
	resolveProtocolTracePath,
} from "./native-traffic-log"

function withTempDir<T>(run: (dir: string) => T): T {
	const dir = mkdtempSync(path.join(tmpdir(), "devo-native-traffic-log-"))
	try {
		return run(dir)
	} finally {
		rmSync(dir, { recursive: true, force: true })
	}
}

const fixedClock = () => new Date("2026-06-27T01:02:03.004Z")

describe("protocol trace env trigger", () => {
	test("isProtocolTraceEnabled accepts 1 and true", () => {
		expect({
			unset: isProtocolTraceEnabled(undefined),
			empty: isProtocolTraceEnabled(" "),
			one: isProtocolTraceEnabled("1"),
			trueValue: isProtocolTraceEnabled("true"),
			TrueValue: isProtocolTraceEnabled("TRUE"),
			zero: isProtocolTraceEnabled("0"),
		}).toEqual({
			unset: false,
			empty: false,
			one: true,
			trueValue: true,
			TrueValue: true,
			zero: false,
		})
	})

	test("does not create or write a log when DEVO_PROTOCOL_TRACE is unset or empty", () => {
		withTempDir((dir) => {
			const logger = createNativeTrafficLoggerFromEnv({
				env: { [DEVO_HOME_ENV]: dir },
				clock: fixedClock,
				pid: 42,
			})

			logger.record({
				direction: "desktop-to-server",
				kind: "request",
				id: 1,
				method: "initialize",
				payload: { jsonrpc: "2.0", id: 1, method: "initialize", params: {} },
			})

			expect({
				state: logger.getState(),
				tracesDirExists: existsSync(path.join(dir, "traces")),
			}).toEqual({
				state: { enabled: false, path: null },
				tracesDirExists: false,
			})
		})

		withTempDir((dir) => {
			const logger = createNativeTrafficLoggerFromEnv({
				env: {
					[DEVO_HOME_ENV]: dir,
					[PROTOCOL_TRACE_ENV]: " ",
				},
				clock: fixedClock,
				pid: 42,
			})

			logger.record({
				direction: "system",
				kind: "closed",
				payload: { error: "transport closed" },
			})

			expect({
				state: logger.getState(),
				tracesDirExists: existsSync(path.join(dir, "traces")),
			}).toEqual({
				state: { enabled: false, path: null },
				tracesDirExists: false,
			})
		})
	})

	test("ignores removed desktop-specific triggers", () => {
		withTempDir((dir) => {
			const logger = createNativeTrafficLoggerFromEnv({
				env: {
					[DEVO_HOME_ENV]: dir,
					DEVO_DESKTOP_ACP_TRAFFIC_LOG: "1",
					DEVO_DESKTOP_ACP_TRAFFIC_LOG_PATH: path.join(dir, "old", "traffic.jsonl"),
					TRAFFIC_LOG_PATH: path.join(dir, "old", "traffic.jsonl"),
				},
				clock: fixedClock,
				pid: 42,
			})

			logger.record({
				direction: "desktop-to-server",
				kind: "request",
				id: 1,
				method: "initialize",
				payload: { jsonrpc: "2.0", id: 1, method: "initialize", params: {} },
			})

			expect({
				state: logger.getState(),
				oldDirExists: existsSync(path.join(dir, "old")),
			}).toEqual({
				state: { enabled: false, path: null },
				oldDirExists: false,
			})
		})
	})

	test("writes JSONL to DEVO_HOME/traces when DEVO_PROTOCOL_TRACE is enabled", () => {
		withTempDir((dir) => {
			const expectedPath = path.join(dir, "traces", "protocol-42-20260627T010203Z.ndjsonl")
			const logger = createNativeTrafficLoggerFromEnv({
				env: {
					[DEVO_HOME_ENV]: dir,
					[PROTOCOL_TRACE_ENV]: "1",
				},
				clock: fixedClock,
				pid: 42,
			})

			logger.record({
				direction: "system",
				kind: "closed",
				payload: { error: "transport closed" },
			})

			const lines = readFileSync(expectedPath, "utf-8").trim().split("\n")
			expect({
				state: logger.getState(),
				entry: JSON.parse(lines[0]),
			}).toEqual({
				state: { enabled: true, path: expectedPath },
				entry: {
					timestamp: "2026-06-27T01:02:03.004Z",
					direction: "system",
					kind: "closed",
					payload: { error: "transport closed" },
				},
			})
		})
	})

	test("writes JSONL to DEVO_PROTOCOL_TRACE_FILE when provided", () => {
		withTempDir((dir) => {
			const logPath = path.join(dir, "custom", "trace.ndjsonl")
			const logger = createNativeTrafficLoggerFromEnv({
				env: {
					[DEVO_HOME_ENV]: dir,
					[PROTOCOL_TRACE_ENV]: "true",
					[PROTOCOL_TRACE_FILE_ENV]: ` ${logPath} `,
				},
				clock: fixedClock,
				pid: 42,
			})

			logger.record({
				direction: "system",
				kind: "closed",
				payload: { error: "transport closed" },
			})

			const lines = readFileSync(logPath, "utf-8").trim().split("\n")
			expect({
				state: logger.getState(),
				entry: JSON.parse(lines[0]),
			}).toEqual({
				state: { enabled: true, path: logPath },
				entry: {
					timestamp: "2026-06-27T01:02:03.004Z",
					direction: "system",
					kind: "closed",
					payload: { error: "transport closed" },
				},
			})
		})
	})

	test("falls back to temp devo-traces when DEVO_HOME is invalid", () => {
		const logPath = resolveProtocolTracePath({
			env: {
				[DEVO_HOME_ENV]: path.join(tmpdir(), "missing-devo-home-for-trace"),
				[PROTOCOL_TRACE_ENV]: "1",
			},
			clock: fixedClock,
			pid: 99,
		})

		expect(logPath).toMatch(/devo-traces[\\/]protocol-99-20260627T010203Z\.ndjsonl$/)
	})

	test("findDevoHome honors DEVO_HOME and defaults to ~/.devo", () => {
		withTempDir((dir) => {
			expect(findDevoHome({ [DEVO_HOME_ENV]: dir })).toBe(path.resolve(dir))
		})
	})

	test("formatProtocolTraceTimestamp matches server naming", () => {
		expect(formatProtocolTraceTimestamp(fixedClock())).toBe("20260627T010203Z")
	})
})
