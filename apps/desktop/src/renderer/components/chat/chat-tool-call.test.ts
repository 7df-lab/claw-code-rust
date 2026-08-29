import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"
import { buildBashTerminalOutput, getToolSubtitle, stripShellEnvelope } from "./chat-tool-call"

const elapsedHookSource = readFileSync(new URL("../../hooks/use-elapsed-time.ts", import.meta.url), "utf8")
const chatToolCallSource = readFileSync(new URL("./chat-tool-call.tsx", import.meta.url), "utf8")
const rendererCssSource = readFileSync(new URL("../../index.css", import.meta.url), "utf8")

describe("buildBashTerminalOutput", () => {
	test("joins command and output into a single terminal block", () => {
		expect({
			plain: buildBashTerminalOutput("bun test", "ok\nDone", undefined),
			echoed: buildBashTerminalOutput("bun test", "$ bun test\nok", undefined),
			errorPreferred: buildBashTerminalOutput("bun test", "partial", "boom"),
			pending: buildBashTerminalOutput("bun test", undefined, undefined),
			noCommand: buildBashTerminalOutput(undefined, "plain output", undefined),
		}).toEqual({
			plain: "$ bun test\nok\nDone",
			echoed: "$ bun test\nok",
			errorPreferred: "$ bun test\nboom",
			pending: "$ bun test",
			noCommand: "plain output",
		})
	})

	test("truncates very long output", () => {
		const truncated = buildBashTerminalOutput(undefined, "x".repeat(6000), undefined)
		expect({
			endsWithMarker: truncated.endsWith("... (truncated)"),
			length: truncated.length,
		}).toEqual({
			endsWithMarker: true,
			length: 5000 + "\n... (truncated)".length,
		})
	})
})

describe("stripShellEnvelope", () => {
	test("strips the shell result envelope from tool output", () => {
		const envelope = JSON.stringify({
			output: "",
			command: "ls",
			exit: 0,
			description: "List files",
			cwd: "/repo",
			yield_time_ms: 1000,
		})
		expect({
			envelopeOnly: stripShellEnvelope(envelope),
			stdoutPlusEnvelope: stripShellEnvelope(`hello\nworld\n${envelope}`),
			plainOutput: stripShellEnvelope("just text"),
			otherJson: stripShellEnvelope('{"foo": 1}'),
			envelopeWithOutput: stripShellEnvelope(
				JSON.stringify({ output: "files", command: "ls", exit: 0 }),
			),
			cmdEnvelope: stripShellEnvelope(JSON.stringify({ output: "ok", cmd: "ls", exit: 0 })),
		}).toEqual({
			envelopeOnly: "",
			stdoutPlusEnvelope: "hello\nworld",
			plainOutput: "just text",
			otherJson: '{"foo": 1}',
			envelopeWithOutput: "files",
			cmdEnvelope: "ok",
		})
	})
})

describe("getToolSubtitle", () => {
	test("shows read paths relative to the project root", () => {
		expect(
			getToolSubtitle(
				{
					callID: "call-1",
					id: "tool-1",
					tool: "read",
					type: "tool",
					state: {
						input: { filePath: "C:\\Users\\lenovo\\Desktop\\devo\\apps\\desktop\\src\\main.ts" },
						status: "completed",
						time: { end: 1, start: 0 },
						output: "",
					},
				} as any,
				{ projectRoot: "C:\\Users\\lenovo\\Desktop\\devo" },
			),
		).toBe("apps/desktop/src/main.ts")
	})

	test("shows write paths relative to the project root", () => {
		expect(
			getToolSubtitle(
				{
					callID: "call-1",
					id: "tool-1",
					tool: "write",
					type: "tool",
					state: {
						input: { path: "C:\\Users\\lenovo\\Desktop\\devo\\README.md" },
						status: "completed",
						time: { end: 1, start: 0 },
						output: "",
					},
				} as any,
				{ projectRoot: "C:\\Users\\lenovo\\Desktop\\devo" },
			),
		).toBe("README.md")
	})

	test("shows apply_patch paths from patch input", () => {
		expect(
			getToolSubtitle(
				{
					callID: "call-1",
					id: "tool-1",
					tool: "apply_patch",
					type: "tool",
					state: {
						input: {
							patch: `*** Begin Patch
*** Update File: C:\\Users\\lenovo\\Desktop\\devo\\apps\\desktop\\src\\main.ts
@@
*** End Patch`,
						},
						status: "completed",
						time: { end: 1, start: 0 },
						output: "",
					},
				} as any,
				{ projectRoot: "C:\\Users\\lenovo\\Desktop\\devo" },
			),
		).toBe("apps/desktop/src/main.ts")
	})
})

describe("read tool output density source", () => {
	test("overrides CodeBlock internal text sizing for read output", () => {
		expect({
			readClass: chatToolCallSource.includes("devo-read-output"),
			preRule: rendererCssSource.includes(".devo-read-output pre"),
			codeRule: rendererCssSource.includes(".devo-read-output code"),
			lineHeight: rendererCssSource.includes("line-height: 1.35"),
			preservesWhitespace: rendererCssSource.includes("white-space: pre"),
		}).toEqual({
			readClass: true,
			preRule: true,
			codeRule: true,
			lineHeight: true,
			preservesWhitespace: true,
		})
	})
})

describe("useToolElapsedTime source", () => {
	test("uses tool state time without renderer first-seen timestamps", () => {
		expect({
			usesStateStart: elapsedHookSource.includes("part.state.time"),
			usesFirstSeen: elapsedHookSource.includes("getPartFirstSeenAt"),
		}).toEqual({
			usesStateStart: true,
			usesFirstSeen: false,
		})
	})
})


describe("ChatToolCall memo comparison", () => {
	test("re-renders when the controlled open state changes so rows can expand", () => {
		expect({
			comparesOpen: chatToolCallSource.includes("prev.open !== next.open"),
			comparesTurnError: chatToolCallSource.includes("prev.turnHasError !== next.turnHasError"),
			comparesTurnWorking: chatToolCallSource.includes("prev.turnWorking !== next.turnWorking"),
			hidesSpinnerWhenTurnIdle: chatToolCallSource.includes(
				"turnWorking && (status === \"running\" || status === \"pending\")",
			),
		}).toEqual({
			comparesOpen: true,
			comparesTurnError: true,
			comparesTurnWorking: true,
			hidesSpinnerWhenTurnIdle: true,
		})
	})

	test("labels MCP tools with the server prefix instead of a generic wrench", () => {
		expect({
			mcpPrefix: chatToolCallSource.includes('tool.startsWith("mcp__")'),
			mcpTitle: chatToolCallSource.includes("`MCP · ${label}`"),
		}).toEqual({
			mcpPrefix: true,
			mcpTitle: true,
		})
	})
})
