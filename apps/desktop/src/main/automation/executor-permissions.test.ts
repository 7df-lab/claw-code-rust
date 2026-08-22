import { describe, expect, test } from "bun:test"
import { buildPermissionRuleset } from "./permission-policy"

describe("automation Native permission policy", () => {
	test("unconfigured automation inherits policy without implicit decisions", () => {
		expect(buildPermissionRuleset("default")).toEqual([])
	})

	test("only explicit presets install unattended permission rules", () => {
		expect(buildPermissionRuleset("allow-all")).toContainEqual({
			permission: "*",
			pattern: "*",
			action: "allow",
		})
		expect(buildPermissionRuleset("read-only")).toContainEqual({
			permission: "bash",
			pattern: "*",
			action: "deny",
		})
	})
})
