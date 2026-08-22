import { mkdirSync, writeFileSync } from "node:fs"
import { mkdir, mkdtemp, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { describe, expect, test } from "bun:test"
import { ensureProtocolTypes } from "./protocol-types"

async function tempDesktopDir(): Promise<string> {
	return mkdtemp(join(tmpdir(), "devo-desktop-protocol-types-"))
}

describe("desktop protocol type generation", () => {
	test("skips generation when the runtime schema already exists", async () => {
		const desktopDir = await tempDesktopDir()
		const generatedDir = join(desktopDir, "packages/devo-ai-sdk/src/v2/generated")
		await mkdir(generatedDir, { recursive: true })
		await writeFile(join(generatedDir, "schema.json"), "{}")
		const calls: string[][] = []

		const status = ensureProtocolTypes({
			desktopDir,
			runGenerator: (command, args) => {
				calls.push([command, ...args])
				return { status: 0 }
			},
		})

		expect(status).toBe("present")
		expect(calls).toEqual([])
	})

	test("generates protocol types before Vite resolves SDK imports", async () => {
		const desktopDir = await tempDesktopDir()
		const calls: string[][] = []

		const status = ensureProtocolTypes({
			desktopDir,
			runGenerator: (command, args) => {
				calls.push([command, ...args])
				const generatedDir = join(desktopDir, "packages/devo-ai-sdk/src/v2/generated")
				mkdirSync(generatedDir, { recursive: true })
				writeFileSync(join(generatedDir, "schema.json"), "{}")
				return { status: 0 }
			},
		})

		expect(status).toBe("generated")
		expect(calls).toEqual([
			[
				"cargo",
				"run",
				"--manifest-path",
				"../../Cargo.toml",
				"-p",
				"devo-protocol",
				"--bin",
				"generate-native-ts",
				"--",
				"packages/devo-ai-sdk/src/v2/generated",
			],
		])
	})
})
