import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, relative, resolve } from "node:path";

const generatedDir = resolve("packages/devo-ai-sdk/src/v2/generated");
const tempDir = await mkdtemp(join(tmpdir(), "devo-protocol-types-"));

try {
	const result = spawnSync(
		"cargo",
		[
			"run",
			"--manifest-path",
			"../../Cargo.toml",
			"-p",
			"devo-protocol",
			"--bin",
			"generate-native-ts",
			"--",
			tempDir
		],
		{ stdio: "inherit" }
	);

	if (result.status !== 0) {
		process.exit(result.status ?? 1);
	}

	await assertGeneratedTypesMatch(generatedDir, tempDir);
} finally {
	await rm(tempDir, { recursive: true, force: true });
}

async function assertGeneratedTypesMatch(currentDir, expectedDir) {
	if (!existsSync(currentDir)) {
		throw new Error(`generated protocol types are missing: ${currentDir}`);
	}

	const [currentFiles, expectedFiles] = await Promise.all([
		collectFiles(currentDir),
		collectFiles(expectedDir)
	]);
	const allFiles = [...new Set([...currentFiles, ...expectedFiles])].sort();
	const differences = [];

	for (const file of allFiles) {
		const currentPath = join(currentDir, file);
		const expectedPath = join(expectedDir, file);
		const currentExists = currentFiles.includes(file);
		const expectedExists = expectedFiles.includes(file);

		if (!currentExists || !expectedExists) {
			differences.push(`${file}: ${currentExists ? "unexpected" : "missing"}`);
			continue;
		}

		const [current, expected] = await Promise.all([
			readFile(currentPath),
			readFile(expectedPath)
		]);
		if (!current.equals(expected)) {
			differences.push(`${file}: content differs`);
		}
	}

	if (differences.length > 0) {
		throw new Error(
			`generated protocol types are stale; run bun run gen:protocol-types\n${differences.join("\n")}`
		);
	}
}

async function collectFiles(dir) {
	const files = [];

	async function visit(currentDir) {
		const entries = await readdir(currentDir, { withFileTypes: true });
		entries.sort((left, right) => left.name.localeCompare(right.name));

		for (const entry of entries) {
			const path = join(currentDir, entry.name);
			if (entry.isDirectory()) {
				await visit(path);
			} else if (entry.isFile()) {
				files.push(relative(dir, path));
			}
		}
	}

	await visit(dir);
	return files;
}
