import { describe, expect, test } from "bun:test"
import {
	computeStatsFromUnifiedDiff,
	fileChangeStats,
	fileChangeVerb,
	hasFileChangeExpandableContent,
	isFileChangeAdd,
} from "./file-change-presentation"

describe("fileChangeVerb", () => {
	test("write uses Writing / Added", () => {
		expect({
			running: fileChangeVerb("write", { running: true }),
			done: fileChangeVerb("write", { running: false }),
		}).toEqual({
			running: "Writing",
			done: "Added",
		})
	})

	test("edit and apply_patch use Editing / Edited", () => {
		expect({
			editRunning: fileChangeVerb("edit", { running: true }),
			editDone: fileChangeVerb("edit", { running: false }),
			patchRunning: fileChangeVerb("apply_patch", { running: true }),
			patchDone: fileChangeVerb("apply_patch", { running: false }),
		}).toEqual({
			editRunning: "Editing",
			editDone: "Edited",
			patchRunning: "Editing",
			patchDone: "Edited",
		})
	})

	test("explicit changeType overrides the tool name", () => {
		expect({
			writeUpdate: fileChangeVerb("write", {
				running: false,
				input: { changeType: "update" },
			}),
			editAdd: fileChangeVerb("edit", {
				running: false,
				input: { changeType: "add" },
			}),
		}).toEqual({
			writeUpdate: "Edited",
			editAdd: "Added",
		})
	})
})

describe("isFileChangeAdd", () => {
	test("prefers changeType over tool name", () => {
		expect({
			writeAdd: isFileChangeAdd("write", { changeType: "add" }),
			writeUpdate: isFileChangeAdd("write", { changeType: "update" }),
			editDefault: isFileChangeAdd("edit", {}),
			writeDefault: isFileChangeAdd("write", {}),
		}).toEqual({
			writeAdd: true,
			writeUpdate: false,
			editDefault: false,
			writeDefault: true,
		})
	})
})

describe("computeStatsFromUnifiedDiff", () => {
	test("counts +/− lines and skips file headers", () => {
		const patch = [
			"diff --git a/foo.ts b/foo.ts",
			"--- a/foo.ts",
			"+++ b/foo.ts",
			"@@ -1,3 +1,4 @@",
			" context",
			"-old",
			"+new",
			"+also",
		].join("\n")
		expect(computeStatsFromUnifiedDiff(patch)).toEqual({
			additions: 2,
			deletions: 1,
		})
	})
})

describe("fileChangeStats", () => {
	test("prefers unifiedDiff over old/new strings", () => {
		expect(
			fileChangeStats("edit", {
				unifiedDiff: "@@\n+a\n-b\n",
				oldString: "x\ny",
				newString: "x",
			}),
		).toEqual({ additions: 1, deletions: 1 })
	})

	test("counts Add content as additions only", () => {
		expect(
			fileChangeStats("write", {
				changeType: "add",
				content: "one\ntwo\nthree",
			}),
		).toEqual({ additions: 3, deletions: 0 })
	})

	test("falls back to old/new string set diff", () => {
		expect(
			fileChangeStats("edit", {
				oldString: "a\nb\nc",
				newString: "a\nx\nc",
			}),
		).toEqual({ additions: 1, deletions: 1 })
	})

	test("returns undefined when there is nothing to measure", () => {
		expect(fileChangeStats("edit", { path: "foo.ts" })).toBeUndefined()
		expect(fileChangeStats("read", { content: "x" })).toBeUndefined()
	})
})

describe("hasFileChangeExpandableContent", () => {
	test("detects old/new, unifiedDiff, and content", () => {
		expect({
			oldNew: hasFileChangeExpandableContent("edit", {
				oldString: "a",
				newString: "b",
			}),
			diff: hasFileChangeExpandableContent("edit", {
				unifiedDiff: "@@\n+a\n",
			}),
			content: hasFileChangeExpandableContent("write", { content: "hi" }),
			empty: hasFileChangeExpandableContent("edit", { path: "x" }),
			patchOutput: hasFileChangeExpandableContent(
				"apply_patch",
				{},
				"diff --git a/a b/a\n+line\n",
			),
		}).toEqual({
			oldNew: true,
			diff: true,
			content: true,
			empty: false,
			patchOutput: true,
		})
	})
})
