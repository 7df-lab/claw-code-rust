import { describe, expect, test } from "bun:test"
import {
	buildApprovalChoices,
	permissionSummaryLines,
} from "./chat-permission-options"

describe("chat permission options", () => {
	test("matches TUI once + deny for minimal scopes", () => {
		const choices = buildApprovalChoices({
			id: "approval-1",
			requestID: "approval-1",
			sessionID: "session-1",
			permission: "write: src/main.rs",
			metadata: { availableScopes: ["once"] },
		})
		expect(choices.map((choice) => choice.label)).toEqual(["Allow once", "Deny"])
	})

	test("includes contextual session and prefix-persist options", () => {
		const choices = buildApprovalChoices({
			id: "approval-1",
			requestID: "approval-1",
			sessionID: "session-1",
			permission: "run git add file.txt",
			metadata: {
				availableScopes: ["once", "session", "commandPrefixPersist"],
				commandPattern: ["git", "add", "*"],
				commandPrefix: ["git", "pull"],
				target: "git add file.txt",
			},
		})
		expect(choices.map((choice) => choice.label)).toEqual([
			"Allow once",
			"Allow for this session · `git add *`",
			"Always allow commands starting with `git pull`",
			"Deny",
		])
	})

	test("does not surface turn or tool scopes in the picker", () => {
		const choices = buildApprovalChoices({
			id: "approval-1",
			requestID: "approval-1",
			sessionID: "session-1",
			permission: "write: hello.txt",
			metadata: {
				availableScopes: ["once", "turn", "tool", "session"],
				path: "C:\\Users\\hello.txt",
			},
		})
		expect(choices.some((choice) => choice.kind === "approve" && choice.scope === "turn")).toBe(
			false,
		)
		expect(choices.some((choice) => choice.kind === "approve" && choice.scope === "tool")).toBe(
			false,
		)
	})

	test("session scope label uses exact filepath for file tools", () => {
		const choices = buildApprovalChoices({
			id: "approval-1",
			requestID: "approval-1",
			sessionID: "session-1",
			permission: "write: C:\\Users\\hello.txt",
			metadata: {
				availableScopes: ["once", "session", "path_prefix"],
				path: "C:\\Users\\lenovo\\Desktop\\hello.txt",
			},
		})
		expect(choices.map((choice) => choice.label)).toEqual([
			"Allow once",
			"Allow for this session · `C:\\Users\\lenovo\\Desktop\\hello.txt`",
			"Allow files under `C:\\Users\\lenovo\\Desktop`",
			"Deny",
		])
	})

	test("uses parent directory for file path prefix labels", () => {
		const choices = buildApprovalChoices({
			id: "approval-1",
			requestID: "approval-1",
			sessionID: "session-1",
			permission: "write: C:\\Users\\hello.txt",
			metadata: {
				availableScopes: ["once", "path_prefix"],
				path: "C:\\Users\\lenovo\\Desktop\\hello.txt",
			},
		})
		expect(choices.map((choice) => choice.label)).toEqual([
			"Allow once",
			"Allow files under `C:\\Users\\lenovo\\Desktop`",
			"Deny",
		])
	})

	test("normalizes legacy snake_case scopes", () => {
		const choices = buildApprovalChoices({
			id: "approval-1",
			requestID: "approval-1",
			sessionID: "session-1",
			permission: "write: hello.txt",
			metadata: {
				availableScopes: ["once", "path_prefix"],
				path: "C:\\Users\\hello.txt",
			},
		})
		expect(choices.map((choice) => choice.label)).toEqual([
			"Allow once",
			"Allow files under `C:\\Users`",
			"Deny",
		])
	})

	test("shows only action summary and agent reason", () => {
		expect(
			permissionSummaryLines({
				id: "approval-1",
				requestID: "approval-1",
				sessionID: "session-1",
				permission: "write: C:\\Users\\hello.txt",
				metadata: {
					tool: "FileWrite",
					path: "C:\\Users\\hello.txt",
					justification: "Need to update the file.",
				},
			}),
		).toEqual({
			title: "write: C:\\Users\\hello.txt",
			reason: "Need to update the file.",
		})
	})
})
