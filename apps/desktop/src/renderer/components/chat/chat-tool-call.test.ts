import { readFileSync } from "node:fs"
import { describe, expect, test } from "bun:test"
import { buildBashTerminalOutput, getToolInfo, getToolSubtitle, parseReadOutput, stripShellEnvelope } from "./chat-tool-call"

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


describe("parseReadOutput", () => {
	test("unwraps stringified Mixed metadata and restores real newlines", () => {
		const body = "<path>hello.py</path>\n<content>\n1: def main():\n2:    print(1)\n</content>"
		const stringified = JSON.stringify({
			output: body,
			preview: "def main",
			truncated: false,
		})
		const parsed = parseReadOutput(stringified)
		expect({
			hasRealNewline: parsed.includes("\n"),
			noLiteralEscape: !parsed.includes("\\n"),
			line1: parsed.includes("1: def main():"),
			line2: parsed.includes("2:    print(1)"),
		}).toEqual({
			hasRealNewline: true,
			noLiteralEscape: true,
			line1: true,
			line2: true,
		})
	})

	test("unescapes literal \\n when they dominate the string", () => {
		const parsed = parseReadOutput(
			'\\n1: def main():\\n2:    name = input(\\"What is your name? \\")',
		)
		expect(parsed.split("\n")).toEqual([
			"",
			"1: def main():",
			'2:    name = input("What is your name? ")',
		])
	})
})

describe("getToolInfo", () => {
	test("labels shell tools as Running while active and Ran when finished", () => {
		expect({
			running: getToolInfo("bash", { running: true }).title,
			ran: getToolInfo("bash", { running: false }).title,
			shellCommand: getToolInfo("shell_command", { running: true }).title,
			execCommand: getToolInfo("exec_command").title,
		}).toEqual({
			running: "Running",
			ran: "Ran",
			shellCommand: "Running",
			execCommand: "Ran",
		})
	})

	test("labels write and edit with Writing/Added and Editing/Edited", () => {
		expect({
			writing: getToolInfo("write", { running: true }).title,
			added: getToolInfo("write", { running: false }).title,
			editing: getToolInfo("edit", { running: true }).title,
			edited: getToolInfo("edit", { running: false }).title,
			patchEditing: getToolInfo("apply_patch", { running: true }).title,
			patchEdited: getToolInfo("apply_patch", { running: false }).title,
		}).toEqual({
			writing: "Writing",
			added: "Added",
			editing: "Editing",
			edited: "Edited",
			patchEditing: "Editing",
			patchEdited: "Edited",
		})
	})

	test("does not use legacy Write/Edit/Patch titles for file-change tools", () => {
		const titles = [
			getToolInfo("write").title,
			getToolInfo("edit").title,
			getToolInfo("apply_patch").title,
		]
		expect(titles).toEqual(["Added", "Edited", "Edited"])
		expect(titles).not.toContain("Write")
		expect(titles).not.toContain("Edit")
		expect(titles).not.toContain("Patch")
	})

	test("shows the shell command after Running instead of a generic title", () => {
		const running = {
			callID: "call-1",
			id: "tool-1",
			tool: "bash",
			type: "tool",
			state: {
				input: { command: "git status", description: "Check git status" },
				status: "running",
				time: { start: 0 },
				title: "Command",
			},
		} as any
		expect({
			title: getToolInfo("bash", { running: true }).title,
			subtitle: getToolSubtitle(running),
			arrayCommand: getToolSubtitle({
				...running,
				tool: "shell_command",
				state: {
					...running.state,
					input: { command: ["git", "status", "--short"] },
				},
			} as any),
			fromRaw: getToolSubtitle({
				...running,
				state: {
					input: {},
					raw: '{"command":"bun test"}',
					status: "pending",
					time: { start: 0 },
					title: "Command",
				},
			} as any),
		}).toEqual({
			title: "Running",
			subtitle: "git status",
			arrayCommand: "git status --short",
			fromRaw: "bun test",
		})
	})

	test("labels question tools even when the SDK fell back to generic tool", () => {
		const input = {
			questions: [{ id: "environment", header: "Environment", question: "Where should this run?" }],
		}
		expect({
			named: getToolInfo("request_user_input").title,
			alias: getToolInfo("question").title,
			generic: getToolInfo("tool", { input }).title,
			subtitle: getToolSubtitle(
				{
					callID: "call-1",
					id: "tool-1",
					tool: "tool",
					type: "tool",
					state: { input, status: "running", time: { start: 0 } },
				} as any,
			),
		}).toEqual({
			named: "Question",
			alias: "Question",
			generic: "Question",
			subtitle: "Where should this run?",
		})
	})

	test("keeps file-change path typography aligned with Read", () => {
		expect({
			noMonoOnFileChangePath: !chatToolCallSource.includes(
				'font-mono text-[12px] text-muted-foreground/50',
			),
			fileChangePathUsesReadMutedClass: /fileChangeRow[\s\S]*?text-muted-foreground\/60/.test(
				chatToolCallSource,
			),
		}).toEqual({
			noMonoOnFileChangePath: true,
			fileChangePathUsesReadMutedClass: true,
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
			gatesBodyWhileControlledClosed: chatToolCallSource.includes("open === false ? null"),
		}).toEqual({
			comparesOpen: true,
			comparesTurnError: true,
			comparesTurnWorking: true,
			hidesSpinnerWhenTurnIdle: true,
			gatesBodyWhileControlledClosed: true,
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
