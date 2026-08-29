import { describe, expect, it, vi } from "vitest"

import { ReferenceSearchSession } from "./reference-search-session"

describe("ReferenceSearchSession", () => {
	it("ignores stale search notifications for a different query", async () => {
		const request = vi.fn(async (method: string, params: unknown) => {
			if (method === "search/start") {
				return {
					snapshot: {
						search_id: "search-1",
						query: "src",
						results: [],
						total_file_match_count: 0,
						scanned_file_count: 0,
						file_search_complete: false,
					},
				}
			}
			throw new Error(`unexpected request ${method}: ${JSON.stringify(params)}`)
		})
		const session = new ReferenceSearchSession(request, "/workspace")
		const snapshots: string[] = []
		session.subscribe((snapshot) => {
			snapshots.push(snapshot.query)
		})

		await session.startOrUpdate("src")
		const handled = session.handleNotification("search/updated", {
			search_id: "search-1",
			query: "old",
			results: [],
			total_file_match_count: 0,
			scanned_file_count: 0,
			file_search_complete: true,
		})

		expect(handled).toBe(true)
		expect(snapshots).toEqual(["src"])
	})

	it("prefers workspace-relative display_name over absolute file_path", async () => {
		const request = vi.fn(async () => ({
			snapshot: {
				search_id: "search-1",
				query: "lib",
				results: [
					{
						kind: "file",
						display_name: "src/lib.rs",
						insert_text: "src/lib.rs",
						file_path: "C:\\workspace\\src\\lib.rs",
					},
				],
				total_file_match_count: 1,
				scanned_file_count: 1,
				file_search_complete: true,
			},
		}))
		const session = new ReferenceSearchSession(request, "C:\\workspace")
		await session.startOrUpdate("lib")

		expect(session.filePaths()).toEqual(["src/lib.rs"])
	})

	it("maps completed file results to paths", async () => {
		const request = vi.fn(async () => ({
			snapshot: {
				search_id: "search-1",
				query: "lib",
				results: [
					{
						kind: "file",
						display_name: "src/lib.rs",
						insert_text: "src/lib.rs",
						file_path: "src/lib.rs",
					},
				],
				total_file_match_count: 1,
				scanned_file_count: 1,
				file_search_complete: false,
			},
		}))
		const session = new ReferenceSearchSession(request, "/workspace")
		await session.startOrUpdate("lib")
		session.handleNotification("search/completed", {
			search_id: "search-1",
			query: "lib",
			results: [
				{
					kind: "file",
					display_name: "src/lib.rs",
					insert_text: "src/lib.rs",
					file_path: "src/lib.rs",
				},
			],
			total_file_match_count: 1,
			scanned_file_count: 1,
			file_search_complete: true,
		})

		expect(session.filePaths()).toEqual(["src/lib.rs"])
	})

	it("normalizes camelCase wire disabled MCP flags", async () => {
		const request = vi.fn(async () => ({
			snapshot: {
				searchId: "search-1",
				query: "docs",
				results: [
					{
						kind: "mcp",
						displayName: "Docs",
						insertText: "@mcp:docs",
						mentionPath: "mcp://server/docs",
						isDisabled: true,
						disabledReason: "Server is disconnected",
					},
				],
				totalFileMatchCount: 0,
				scannedFileCount: 0,
				fileSearchComplete: true,
			},
		}))
		const session = new ReferenceSearchSession(request, "/workspace")
		const snapshot = await session.startOrUpdate("docs")

		expect(snapshot.results).toEqual([
			{
				kind: "mcp",
				display_name: "Docs",
				insert_text: "@mcp:docs",
				description: undefined,
				mention_path: "mcp://server/docs",
				file_path: undefined,
				is_disabled: true,
				disabled_reason: "Server is disconnected",
			},
		])
	})
})
