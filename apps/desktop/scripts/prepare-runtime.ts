import { chmodSync, copyFileSync, existsSync, mkdirSync, rmSync } from "node:fs"
import { homedir } from "node:os"
import { delimiter, dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

interface DefaultSourcePathOptions {
	repoRoot: string
	targetTriple?: string
	platform?: NodeJS.Platform
}

interface StageRuntimeOptions extends DefaultSourcePathOptions {
	devoBin?: string
	desktopDir: string
	hostArch?: NodeJS.Architecture
	hostPlatform?: NodeJS.Platform
	rgBin?: string
	withCodeSearch?: boolean
}

const scriptDir = dirname(fileURLToPath(import.meta.url))
const desktopDir = join(scriptDir, "..")
const repoRoot = join(desktopDir, "..", "..")

export function runtimeBinaryName(name: string, platform: NodeJS.Platform): string {
	return platform === "win32" ? `${name}.exe` : name
}

export function platformForTargetTriple(
	targetTriple: string | undefined,
	fallback: NodeJS.Platform,
): NodeJS.Platform {
	if (!targetTriple) return fallback
	if (targetTriple.includes("pc-windows-msvc")) return "win32"
	if (targetTriple.includes("apple-darwin")) return "darwin"
	if (targetTriple.includes("unknown-linux")) return "linux"
	return fallback
}

export function defaultDevoSourcePath({
	repoRoot,
	targetTriple,
	platform = process.platform,
}: DefaultSourcePathOptions): string {
	const targetParts = targetTriple ? ["target", targetTriple, "release"] : ["target", "release"]
	return join(repoRoot, ...targetParts, runtimeBinaryName("devo", platformForTargetTriple(targetTriple, platform)))
}

export function defaultCodeSearchMcpSourcePath({
	repoRoot,
	targetTriple,
	platform = process.platform,
}: DefaultSourcePathOptions): string {
	const targetParts = targetTriple ? ["target", targetTriple, "release"] : ["target", "release"]
	return join(
		repoRoot,
		...targetParts,
		runtimeBinaryName("devo-code-search-mcp", platformForTargetTriple(targetTriple, platform)),
	)
}

export function stageRuntime(options: StageRuntimeOptions): void {
	const devoSource = options.devoBin ?? defaultDevoSourcePath(options)
	const withCodeSearch = options.withCodeSearch ?? false
	const codeSearchMcpSource = withCodeSearch ? defaultCodeSearchMcpSourcePath(options) : undefined
	const targetPlatform = platformForTargetTriple(options.targetTriple, options.platform ?? process.platform)
	const rgOverride = options.rgBin ?? optionalPath(process.env.DEVO_DESKTOP_RUNTIME_RG_BIN)

	if (
		!rgOverride &&
		options.targetTriple &&
		!targetMatchesHost({
			targetTriple: options.targetTriple,
			platform: options.hostPlatform ?? process.platform,
			arch: options.hostArch ?? process.arch,
		})
	) {
		throw new Error(
			`ripgrep sidecar for cross-target ${options.targetTriple} must be passed with --rg-bin or DEVO_DESKTOP_RUNTIME_RG_BIN`,
		)
	}

	const rgSource =
		rgOverride ??
		findExecutable(runtimeBinaryName("rg", targetPlatform), [
			join(homedir(), ".devo", "bin"),
			...(process.env.PATH ?? "").split(delimiter),
		])

	if (!existsSync(devoSource)) {
		throw new Error(`Devo runtime binary not found at ${devoSource}`)
	}
	if (codeSearchMcpSource && !existsSync(codeSearchMcpSource)) {
		throw new Error(`code_search MCP binary not found at ${codeSearchMcpSource}`)
	}
	if (!rgSource || !existsSync(rgSource)) {
		throw new Error("ripgrep sidecar not found. Install rg or pass --rg-bin <path>.")
	}

	const runtimeBinDir = join(options.desktopDir, "resources", "runtime", "bin")
	rmSync(runtimeBinDir, { recursive: true, force: true })
	mkdirSync(runtimeBinDir, { recursive: true })

	const devoDest = join(runtimeBinDir, runtimeBinaryName("devo", targetPlatform))
	const rgDest = join(runtimeBinDir, runtimeBinaryName("rg", targetPlatform))
	copyExecutable(devoSource, devoDest, targetPlatform)
	copyExecutable(rgSource, rgDest, targetPlatform)
	if (codeSearchMcpSource) {
		const codeSearchMcpDest = join(
			runtimeBinDir,
			runtimeBinaryName("devo-code-search-mcp", targetPlatform),
		)
		copyExecutable(codeSearchMcpSource, codeSearchMcpDest, targetPlatform)
		console.log(`Prepared code_search MCP sidecar: ${codeSearchMcpDest}`)
	}

	console.log(`Prepared Desktop runtime: ${devoDest}`)
	console.log(`Prepared ripgrep sidecar: ${rgDest}`)
}

function targetMatchesHost({
	targetTriple,
	platform,
	arch,
}: {
	targetTriple: string
	platform: NodeJS.Platform
	arch: NodeJS.Architecture
}): boolean {
	const targetPlatform = platformForTargetTriple(targetTriple, platform)
	return targetPlatform === platform && targetTriple.startsWith(`${targetArchName(arch)}-`)
}

function targetArchName(arch: NodeJS.Architecture): string {
	if (arch === "x64") return "x86_64"
	if (arch === "arm64") return "aarch64"
	return arch
}

function copyExecutable(source: string, dest: string, platform: NodeJS.Platform): void {
	copyFileSync(source, dest)
	if (platform !== "win32") chmodSync(dest, 0o755)
}

function findExecutable(name: string, dirs: string[]): string | null {
	for (const dir of dirs) {
		if (!dir) continue
		const candidate = join(dir, name)
		if (existsSync(candidate)) return candidate
	}
	return null
}

function argValue(name: string): string | undefined {
	const index = process.argv.indexOf(name)
	return index >= 0 ? process.argv[index + 1] : undefined
}

function optionalPath(value: string | undefined): string | undefined {
	const trimmed = value?.trim()
	return trimmed ? trimmed : undefined
}

if (import.meta.main) {
	stageRuntime({
		desktopDir,
		repoRoot,
		targetTriple: argValue("--target"),
		platform: process.platform,
		devoBin: argValue("--devo-bin") ?? optionalPath(process.env.DEVO_DESKTOP_RUNTIME_DEVO_BIN),
		rgBin: argValue("--rg-bin"),
		withCodeSearch: process.argv.includes("--with-code-search"),
	})
}
