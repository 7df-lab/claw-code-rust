import { describe, expect, test } from "bun:test"
import { parseDiffFromFile } from "@pierre/diffs"
import {
	buildExpandableGitFileDiff,
	countChangedLinesInFileDiff,
} from "./review-file-diff"

const OLD_LF = ["line1", "line2", "old", "line4", "line5", "line6"].join("\n") + "\n"
const NEW_LF = ["line1", "line2", "new", "line4", "line5", "line6"].join("\n") + "\n"
/** Same content as NEW_LF but CRLF — blows up jsdiff without stripTrailingCr. */
const NEW_CRLF = NEW_LF.replace(/\n/g, "\r\n")

const SMALL_GIT_PATCH = [
	"diff --git a/demo.ts b/demo.ts",
	"index 1111111..2222222 100644",
	"--- a/demo.ts",
	"+++ b/demo.ts",
	"@@ -1,6 +1,6 @@",
	" line1",
	" line2",
	"-old",
	"+new",
	" line4",
	" line5",
	" line6",
].join("\n")

describe("buildExpandableGitFileDiff", () => {
	test("keeps git hunk sizes when sides have CRLF mismatch", () => {
		const fileDiff = buildExpandableGitFileDiff({
			patch: SMALL_GIT_PATCH,
			fileName: "demo.ts",
			oldText: OLD_LF,
			newText: NEW_CRLF,
		})
		expect(fileDiff).not.toBeNull()
		expect(fileDiff?.isPartial).toBe(false)
		expect(countChangedLinesInFileDiff(fileDiff!)).toEqual({
			additions: 1,
			deletions: 1,
		})
	})

	test("jsdiff MultiFileDiff path would count every line changed under CRLF mismatch", () => {
		const reDiff = parseDiffFromFile(
			{ name: "demo.ts", contents: OLD_LF },
			{ name: "demo.ts", contents: NEW_CRLF },
		)
		const changed = countChangedLinesInFileDiff(reDiff)
		expect(changed.additions + changed.deletions).toBeGreaterThan(2)
		expect(changed.additions).toBe(OLD_LF.split("\n").filter(Boolean).length)
	})

	test("returns null for empty patch", () => {
		expect(
			buildExpandableGitFileDiff({
				patch: "   ",
				fileName: "demo.ts",
				oldText: OLD_LF,
				newText: NEW_LF,
			}),
		).toBeNull()
	})
})
