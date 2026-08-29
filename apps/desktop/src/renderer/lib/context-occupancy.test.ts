import { describe, expect, test } from "bun:test"
import {
	occupancyCategoryRows,
	occupancyWindowPercent,
	type ContextOccupancy,
} from "./context-occupancy"

const sampleOccupancy: ContextOccupancy = {
	totalTokens: 100_000,
	contextWindowTokens: 200_000,
	categories: [
		{ id: "base", tokens: 10_000, shareBps: 1000 },
		{ id: "skills", tokens: 5_000, shareBps: 500 },
		{ id: "toolsBuiltin", tokens: 20_000, shareBps: 2000 },
		{ id: "toolsMcp", tokens: 15_000, shareBps: 1500 },
		{ id: "conversation", tokens: 50_000, shareBps: 5000 },
	],
}

describe("occupancyWindowPercent", () => {
	test("returns rounded window fill and clamps missing data to zero", () => {
		expect({
			halfFull: occupancyWindowPercent(sampleOccupancy),
			empty: occupancyWindowPercent(null),
			zeroWindow: occupancyWindowPercent({
				totalTokens: 10,
				contextWindowTokens: 0,
				categories: [],
			}),
		}).toEqual({
			halfFull: 50,
			empty: 0,
			zeroWindow: 0,
		})
	})
})

describe("occupancyCategoryRows", () => {
	test("keeps TUI /status order and fills missing categories with zeros", () => {
		expect(
			occupancyCategoryRows({
				totalTokens: 10_000,
				contextWindowTokens: 100_000,
				categories: [{ id: "conversation", tokens: 10_000, shareBps: 10_000 }],
			}),
		).toEqual([
			{ id: "base", label: "Base", tokens: 0, shareBps: 0, sharePercent: 0 },
			{ id: "skills", label: "Skills", tokens: 0, shareBps: 0, sharePercent: 0 },
			{
				id: "toolsBuiltin",
				label: "Tools (builtin)",
				tokens: 0,
				shareBps: 0,
				sharePercent: 0,
			},
			{ id: "toolsMcp", label: "Tools (MCP)", tokens: 0, shareBps: 0, sharePercent: 0 },
			{
				id: "conversation",
				label: "Conversation",
				tokens: 10_000,
				shareBps: 10_000,
				sharePercent: 100,
			},
		])
	})

	test("maps populated occupancy shares the same way as TUI /status", () => {
		expect(occupancyCategoryRows(sampleOccupancy)).toEqual([
			{ id: "base", label: "Base", tokens: 10_000, shareBps: 1000, sharePercent: 10 },
			{ id: "skills", label: "Skills", tokens: 5_000, shareBps: 500, sharePercent: 5 },
			{
				id: "toolsBuiltin",
				label: "Tools (builtin)",
				tokens: 20_000,
				shareBps: 2000,
				sharePercent: 20,
			},
			{
				id: "toolsMcp",
				label: "Tools (MCP)",
				tokens: 15_000,
				shareBps: 1500,
				sharePercent: 15,
			},
			{
				id: "conversation",
				label: "Conversation",
				tokens: 50_000,
				shareBps: 5000,
				sharePercent: 50,
			},
		])
	})
})
