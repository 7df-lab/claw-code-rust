import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"

const watcherSource = readFileSync(new URL("./notification-watcher.ts", import.meta.url), "utf8")
const managerSource = readFileSync(new URL("./devo-manager.ts", import.meta.url), "utf8")
const sdkPackageJson = readFileSync(
	new URL("../../packages/devo-ai-sdk/package.json", import.meta.url),
	"utf8",
)

/**
 * A stale singleton server (older build holding `~/.devo/server.lock`) makes our
 * stdio child proxy to it, so this build's schema rejects its traffic. The
 * watcher must detect that specific failure and recycle the server instead of
 * retrying against incompatible code forever.
 */
describe("protocol mismatch recovery", () => {
	test("notification watcher recycles the server on ProtocolValidationError", () => {
		expect({
			importsError: watcherSource.includes(
				'from "@devo-ai/sdk/v2/protocol-validation"',
			),
			detectsError: watcherSource.includes("isProtocolValidationError(err)"),
			detectorChecksName:
				watcherSource.includes('.name === "ProtocolValidationError"'),
			callsRecycle: watcherSource.includes("recycleServerForProtocolMismatch("),
			exitsAfterRecycle: watcherSource.includes("if (recycled) return"),
		}).toEqual({
			importsError: true,
			detectsError: true,
			detectorChecksName: true,
			callsRecycle: true,
			exitsAfterRecycle: true,
		})
	})

	test("recycle shuts the singleton down and restarts the managed server", () => {
		expect({
			exportsRecycle: managerSource.includes(
				"export async function recycleServerForProtocolMismatch",
			),
			shutsSingletonDown: managerSource.includes('["server", "--shutdown"]'),
			restartsServer: managerSource.includes("await restartServer()"),
			cooldownGuardsLoop: managerSource.includes("PROTOCOL_RECYCLE_COOLDOWN_MS"),
			cooldownSkips: managerSource.includes("return false"),
		}).toEqual({
			exportsRecycle: true,
			shutsSingletonDown: true,
			restartsServer: true,
			cooldownGuardsLoop: true,
			cooldownSkips: true,
		})
	})

	test("SDK exports the protocol-validation module for main-process consumers", () => {
		expect(sdkPackageJson.includes('"./v2/protocol-validation"')).toBe(true)
	})
})
