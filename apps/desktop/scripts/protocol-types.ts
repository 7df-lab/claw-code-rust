import { spawnSync } from "node:child_process"
import { existsSync } from "node:fs"
import { join } from "node:path"
import type { Plugin } from "vite"

const GENERATED_DIR = "packages/devo-ai-sdk/src/v2/generated"
const GENERATED_SCHEMA = join(GENERATED_DIR, "schema.json")
const GENERATE_PROTOCOL_TYPES_ARGS = [
	"run",
	"--manifest-path",
	"../../Cargo.toml",
	"-p",
	"devo-protocol",
	"--bin",
	"generate-native-ts",
	"--",
	GENERATED_DIR,
]

export type ProtocolTypesStatus = "present" | "generated"

type GeneratorResult = {
	status: number | null
}

type GeneratorRunner = (
	command: string,
	args: string[],
	options: { desktopDir: string },
) => GeneratorResult

type EnsureProtocolTypesOptions = {
	desktopDir?: string
	runGenerator?: GeneratorRunner
}

export function ensureProtocolTypes({
	desktopDir = process.cwd(),
	runGenerator = defaultRunGenerator,
}: EnsureProtocolTypesOptions = {}): ProtocolTypesStatus {
	const schemaPath = join(desktopDir, GENERATED_SCHEMA)
	if (existsSync(schemaPath)) return "present"

	const result = runGenerator("cargo", GENERATE_PROTOCOL_TYPES_ARGS, { desktopDir })
	if (result.status !== 0) {
		throw new Error(`failed to generate desktop protocol types with status ${result.status ?? "null"}`)
	}
	if (!existsSync(schemaPath)) {
		throw new Error(`generated desktop protocol schema is missing: ${schemaPath}`)
	}

	return "generated"
}

export function protocolTypesPlugin({ desktopDir }: EnsureProtocolTypesOptions = {}): Plugin {
	return {
		name: "devo-protocol-types",
		enforce: "pre",
		config() {
			ensureProtocolTypes({ desktopDir })
		},
	}
}

function defaultRunGenerator(
	command: string,
	args: string[],
	{ desktopDir }: { desktopDir: string },
): GeneratorResult {
	return spawnSync(command, args, { cwd: desktopDir, stdio: "inherit" })
}
