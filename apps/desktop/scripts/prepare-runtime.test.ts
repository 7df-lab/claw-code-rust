import { describe, expect, test } from "bun:test"
import { existsSync, mkdirSync, mkdtempSync, readFileSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { defaultDevoSourcePath, runtimeBinaryName, stageRuntime } from "./prepare-runtime"

describe("prepare-runtime helpers", () => {
	test("uses platform executable names", () => {
		expect({
			darwin: runtimeBinaryName("devo", "darwin"),
			linux: runtimeBinaryName("devo", "linux"),
			win32: runtimeBinaryName("devo", "win32"),
		}).toEqual({
			darwin: "devo",
			linux: "devo",
			win32: "devo.exe",
		})
	})

	test("resolves cargo release output by target triple", () => {
		expect(
			defaultDevoSourcePath({
				repoRoot: "/repo",
				targetTriple: "x86_64-apple-darwin",
				platform: "darwin",
			}),
		).toBe(join("/repo", "target", "x86_64-apple-darwin", "release", "devo"))
	})

	test("derives Windows executable names from target triples", () => {
		expect(
			defaultDevoSourcePath({
				repoRoot: "/repo",
				targetTriple: "x86_64-pc-windows-msvc",
				platform: "darwin",
			}),
		).toBe(join("/repo", "target", "x86_64-pc-windows-msvc", "release", "devo.exe"))
	})

	test("requires explicit ripgrep sidecar for cross-target staging", () => {
		const root = mkdtempSync(join(tmpdir(), "devo-runtime-test-"))
		const repoRoot = join(root, "repo")
		const desktopDir = join(root, "desktop")
		const targetDir = join(repoRoot, "target", "aarch64-apple-darwin", "release")
		mkdirSync(targetDir, { recursive: true })
		mkdirSync(desktopDir, { recursive: true })
		writeFileSync(join(targetDir, "devo"), "")
		writeFileSync(join(targetDir, "devo-code-search-mcp"), "")

		expect(() =>
			stageRuntime({
				desktopDir,
				repoRoot,
				targetTriple: "aarch64-apple-darwin",
				hostPlatform: "linux",
				hostArch: "x64",
			}),
		).toThrow("ripgrep sidecar for cross-target aarch64-apple-darwin must be passed")
		expect(existsSync(join(desktopDir, "resources", "runtime", "bin"))).toBe(false)
	})

	test("stages Devo and ripgrep sidecars into the desktop runtime directory", () => {
		const root = mkdtempSync(join(tmpdir(), "devo-runtime-test-"))
		const desktopDir = join(root, "desktop")
		const sourceDir = join(root, "source")
		const releaseDir = join(root, "target", "release")
		const devoBin = join(sourceDir, "devo")
		const rgBin = join(sourceDir, "rg")
		mkdirSync(sourceDir, { recursive: true })
		mkdirSync(releaseDir, { recursive: true })
		writeFileSync(devoBin, "devo")
		writeFileSync(rgBin, "rg")
		writeFileSync(join(releaseDir, "devo-code-search-mcp"), "mcp")

		stageRuntime({
			desktopDir,
			repoRoot: root,
			platform: "darwin",
			devoBin,
			rgBin,
		})

		expect({
			devo: readFileSync(join(desktopDir, "resources", "runtime", "bin", "devo"), "utf8"),
			mcp: readFileSync(
				join(desktopDir, "resources", "runtime", "bin", "devo-code-search-mcp"),
				"utf8",
			),
			rg: readFileSync(join(desktopDir, "resources", "runtime", "bin", "rg"), "utf8"),
		}).toEqual({
			devo: "devo",
			mcp: "mcp",
			rg: "rg",
		})
	})
})
