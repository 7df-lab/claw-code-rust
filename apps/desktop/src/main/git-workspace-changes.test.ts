import { mkdtemp, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import path from "node:path"
import { afterEach, describe, expect, test } from "bun:test"
import simpleGit from "simple-git"
import {
	localWorkspaceChangesSummary,
	localWorkspaceFilePatch,
} from "./git-workspace-changes"

const temps: string[] = []

afterEach(async () => {
	// Best-effort; temp dirs are small.
	temps.length = 0
})

async function initRepo(): Promise<string> {
	const dir = await mkdtemp(path.join(tmpdir(), "devo-local-changes-"))
	temps.push(dir)
	const git = simpleGit({ baseDir: dir })
	await git.init()
	await git.addConfig("user.email", "test@example.com")
	await git.addConfig("user.name", "Test")
	await writeFile(path.join(dir, "tracked.txt"), "before\n")
	await git.add("tracked.txt")
	await git.commit("initial")
	return dir
}

describe("localWorkspaceChangesSummary", () => {
	test("lists uncommitted modifications without unified_diff", async () => {
		const dir = await initRepo()
		await writeFile(path.join(dir, "tracked.txt"), "after\n")
		await writeFile(path.join(dir, "new.txt"), "fresh\n")

		const view = await localWorkspaceChangesSummary(dir, "uncommitted")
		expect(view.unified_diff).toBeNull()
		expect(view.files.map((f) => f.path).sort()).toEqual(["new.txt", "tracked.txt"])
		expect(view.stats.files_changed).toBe(2)
	})

	test("loads a single-file patch on demand", async () => {
		const dir = await initRepo()
		await writeFile(path.join(dir, "tracked.txt"), "after\n")

		const patch = await localWorkspaceFilePatch(dir, "uncommitted", "tracked.txt")
		expect(patch).toContain("tracked.txt")
		expect(patch).toContain("+after")
		expect(patch).toContain("-before")
	})

	test("uses fileStatus=untracked without ls-files round trip", async () => {
		const dir = await initRepo()
		await writeFile(path.join(dir, "fresh.txt"), "new\n")

		const patch = await localWorkspaceFilePatch(dir, "uncommitted", "fresh.txt", {
			fileStatus: "untracked",
		})
		expect(patch).toContain("fresh.txt")
		expect(patch).toContain("+new")
	})

	test("diffs a single file against a turn checkpoint commit", async () => {
		const dir = await initRepo()
		const git = simpleGit({ baseDir: dir })
		const checkpoint = (await git.revparse(["HEAD"])).trim()
		await writeFile(path.join(dir, "tracked.txt"), "turn-change\n")

		const patch = await localWorkspaceFilePatch(dir, "turn", "tracked.txt", {
			checkpointId: checkpoint,
		})
		expect(patch).toContain("tracked.txt")
		expect(patch).toContain("+turn-change")
		expect(patch).toContain("-before")
	})
})