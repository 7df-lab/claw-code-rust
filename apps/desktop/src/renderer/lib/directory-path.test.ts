import { describe, expect, test } from "bun:test"
import { directoriesMatch, normalizeDirectoryPath } from "./directory-path"

describe("directory path matching", () => {
	test("treats slash, case, and trailing-separator variants as the same directory", () => {
		expect(normalizeDirectoryPath("C:\\Users\\lenovo\\Desktop\\devo\\")).toBe(
			"c:/users/lenovo/desktop/devo",
		)
		expect(
			directoriesMatch("C:\\Users\\lenovo\\Desktop\\devo", "C:/Users/lenovo/Desktop/devo/"),
		).toBe(true)
		expect(directoriesMatch("/repo/alpha", "/repo/beta")).toBe(false)
	})
})
