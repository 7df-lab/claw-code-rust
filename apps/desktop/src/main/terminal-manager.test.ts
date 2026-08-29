import { describe, expect, test } from "bun:test";
import {
	DesktopTerminalManager,
	type TerminalCreateProcessOptions,
	type TerminalProcess,
} from "./terminal-manager";

class FakeTerminalProcess implements TerminalProcess {
	readonly writes: string[] = [];
	readonly resizes: { cols: number; rows: number }[] = [];
	readonly kills: string[] = [];
	private dataHandlers: ((data: string) => void)[] = [];
	private exitHandlers: ((event: {
		exitCode: number;
		signal?: number;
	}) => void)[] = [];

	write(data: string): void {
		this.writes.push(data);
	}

	resize(cols: number, rows: number): void {
		this.resizes.push({ cols, rows });
	}

	kill(signal?: string): void {
		this.kills.push(signal ?? "SIGTERM");
	}

	onData(callback: (data: string) => void): void {
		this.dataHandlers.push(callback);
	}

	onExit(
		callback: (event: { exitCode: number; signal?: number }) => void,
	): void {
		this.exitHandlers.push(callback);
	}

	emitData(data: string): void {
		for (const handler of this.dataHandlers) {
			handler(data);
		}
	}

	emitExit(event: { exitCode: number; signal?: number }): void {
		for (const handler of this.exitHandlers) {
			handler(event);
		}
	}
}

describe("DesktopTerminalManager", () => {
	test("creates a terminal session and forwards output to subscribers", async () => {
		const createOptions: TerminalCreateProcessOptions[] = [];
		const process = new FakeTerminalProcess();
		const received: { id: string; data: string }[] = [];
		const manager = new DesktopTerminalManager({
			createProcess: (options) => {
				createOptions.push(options);
				return process;
			},
			defaultCwd: "/Users/tester",
			env: {
				PATH: "/usr/bin",
				SHELL: "/bin/zsh",
				ZSH_TMUX_AUTOSTARTED: "true",
				ZSH_TMUX_AUTOSTART: "false",
			},
			platform: "darwin",
			idFactory: () => "terminal-1",
		});

		manager.onData((id, data) => received.push({ id, data }));
		const session = await manager.create({
			cwd: "/repo/devo",
			cols: 100,
			rows: 30,
		});

		expect(session).toEqual({
			id: "terminal-1",
			cwd: "/repo/devo",
			shell: "/bin/zsh",
			cols: 100,
			rows: 30,
		});
		expect(createOptions).toEqual([
			{
				shell: "/bin/zsh",
				args: ["-l"],
				cwd: "/repo/devo",
				env: {
					DISABLE_AUTO_UPDATE: "true",
					PATH: "/usr/bin",
					POWERSHELL_UPDATECHECK: "Off",
					SHELL: "/bin/zsh",
					TERM: "xterm-256color",
					COLORTERM: "truecolor",
				},
				cols: 100,
				rows: 30,
			},
		]);

		process.emitData("hello\r\n");
		expect(received).toEqual([{ id: "terminal-1", data: "hello\r\n" }]);
	});

	test("uses the Windows Terminal default PowerShell Core profile", async () => {
		const createOptions: TerminalCreateProcessOptions[] = [];
		const process = new FakeTerminalProcess();
		const settingsPath =
			"C:\\Users\\tester\\AppData\\Local\\Packages\\Microsoft.WindowsTerminal_8wekyb3d8bbwe\\LocalState\\settings.json";
		const manager = new DesktopTerminalManager({
			createProcess: (options) => {
				createOptions.push(options);
				return process;
			},
			defaultCwd: "C:\\Users\\tester",
			env: {
				LOCALAPPDATA: "C:\\Users\\tester\\AppData\\Local",
				PATH: "C:\\Program Files\\PowerShell\\7",
				PATHEXT: ".EXE",
			},
			platform: "win32",
			idFactory: () => "terminal-pwsh",
			exists: (path) => {
				return (
					path === settingsPath ||
					path === "C:\\Program Files\\PowerShell\\7\\pwsh.exe"
				);
			},
			readFile: () =>
				JSON.stringify({
					defaultProfile: "{574e775e-4f2a-5b96-ac1e-a2962a402336}",
					profiles: {
						list: [
							{
								guid: "{574e775e-4f2a-5b96-ac1e-a2962a402336}",
								name: "PowerShell",
								source: "Windows.Terminal.PowershellCore",
							},
						],
					},
				}),
		});

		const session = await manager.create({
			cwd: "C:\\repo\\devo",
			cols: 120,
			rows: 40,
		});

		expect(session).toEqual({
			id: "terminal-pwsh",
			cwd: "C:\\repo\\devo",
			shell: "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
			cols: 120,
			rows: 40,
		});
		expect(createOptions).toEqual([
			{
				shell: "C:\\Program Files\\PowerShell\\7\\pwsh.exe",
				args: ["-NoLogo"],
				cwd: "C:\\repo\\devo",
				env: {
					COLORTERM: "truecolor",
					DISABLE_AUTO_UPDATE: "true",
					LOCALAPPDATA: "C:\\Users\\tester\\AppData\\Local",
					PATH: "C:\\Program Files\\PowerShell\\7",
					PATHEXT: ".EXE",
					POWERSHELL_UPDATECHECK: "Off",
					TERM: "xterm-256color",
				},
				cols: 120,
				rows: 40,
			},
		]);
	});

	test("parses Windows Terminal commandline profiles with env vars and args", async () => {
		const createOptions: TerminalCreateProcessOptions[] = [];
		const process = new FakeTerminalProcess();
		const settingsPath =
			"C:\\Users\\tester\\AppData\\Local\\Packages\\Microsoft.WindowsTerminal_8wekyb3d8bbwe\\LocalState\\settings.json";
		const manager = new DesktopTerminalManager({
			createProcess: (options) => {
				createOptions.push(options);
				return process;
			},
			defaultCwd: "C:\\Users\\tester",
			env: {
				LOCALAPPDATA: "C:\\Users\\tester\\AppData\\Local",
				SystemRoot: "C:\\Windows",
			},
			platform: "win32",
			idFactory: () => "terminal-windows-powershell",
			exists: (path) => path === settingsPath,
			readFile: () =>
				JSON.stringify({
					defaultProfile: "{61c54bbd-c2c6-5271-96e7-009a87ff44bf}",
					profiles: {
						list: [
							{
								commandline:
									'"%SystemRoot%\\System32\\WindowsPowerShell\\v1.0\\powershell.exe" -NoLogo',
								guid: "{61c54bbd-c2c6-5271-96e7-009a87ff44bf}",
								name: "Windows PowerShell",
							},
						],
					},
				}),
		});

		await manager.create({ cwd: "C:\\repo\\devo" });

		expect(createOptions).toEqual([
			{
				shell:
					"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
				args: ["-NoLogo"],
				cwd: "C:\\repo\\devo",
				env: {
					COLORTERM: "truecolor",
					DISABLE_AUTO_UPDATE: "true",
					LOCALAPPDATA: "C:\\Users\\tester\\AppData\\Local",
					POWERSHELL_UPDATECHECK: "Off",
					SystemRoot: "C:\\Windows",
					TERM: "xterm-256color",
				},
				cols: 80,
				rows: 24,
			},
		]);
	});

	test("falls back through PATH shells before ComSpec on Windows", async () => {
		const createOptions: TerminalCreateProcessOptions[] = [];
		const firstProcess = new FakeTerminalProcess();
		const secondProcess = new FakeTerminalProcess();
		const processQueue = [firstProcess, secondProcess];
		const manager = new DesktopTerminalManager({
			createProcess: (options) => {
				createOptions.push(options);
				const process = processQueue.shift();
				if (!process) throw new Error("expected a queued terminal process");
				return process;
			},
			defaultCwd: "C:\\Users\\tester",
			env: {
				ComSpec: "C:\\Windows\\System32\\cmd.exe",
				PATH: "C:\\Program Files\\PowerShell\\7;C:\\Windows\\System32\\WindowsPowerShell\\v1.0",
				PATHEXT: ".EXE",
			},
			platform: "win32",
			idFactory: (() => {
				let next = 0;
				return () => `terminal-fallback-${++next}`;
			})(),
			exists: (path) => {
				return path === "C:\\Program Files\\PowerShell\\7\\pwsh.exe";
			},
			readFile: () => {
				throw new Error("Windows Terminal settings should not be read");
			},
		});

		await manager.create({});
		manager.closeAll();

		const powershellManager = new DesktopTerminalManager({
			createProcess: (options) => {
				createOptions.push(options);
				return new FakeTerminalProcess();
			},
			defaultCwd: "C:\\Users\\tester",
			env: {
				ComSpec: "C:\\Windows\\System32\\cmd.exe",
				PATH: "C:\\Program Files\\PowerShell\\7;C:\\Windows\\System32\\WindowsPowerShell\\v1.0",
				PATHEXT: ".EXE",
			},
			platform: "win32",
			idFactory: () => "terminal-fallback-powershell",
			exists: (path) => {
				return (
					path ===
					"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe"
				);
			},
			readFile: () => {
				throw new Error("Windows Terminal settings should not be read");
			},
		});

		await powershellManager.create({});

		expect(createOptions.map((options) => options.shell)).toEqual([
			"C:\\Program Files\\PowerShell\\7\\pwsh.exe",
			"C:\\Windows\\System32\\WindowsPowerShell\\v1.0\\powershell.exe",
		]);
		expect(createOptions.map((options) => options.args)).toEqual([
			["-NoLogo"],
			["-NoLogo"],
		]);
	});

	test("writes, resizes, and removes sessions when they close", async () => {
		const createOptions: TerminalCreateProcessOptions[] = [];
		const process = new FakeTerminalProcess();
		const exited: { id: string; exitCode: number; signal?: number }[] = [];
		const manager = new DesktopTerminalManager({
			createProcess: (options) => {
				createOptions.push(options);
				return process;
			},
			defaultCwd: "/Users/tester",
			env: { ComSpec: "C:\\Windows\\System32\\cmd.exe" },
			platform: "win32",
			idFactory: () => "terminal-2",
		});

		manager.onExit((id, event) => exited.push({ id, ...event }));
		await manager.create({ cols: 80, rows: 24 });
		manager.write("terminal-2", "pwd\r");
		manager.resize("terminal-2", 120, 40);

		expect(process.writes).toEqual(["pwd\r"]);
		expect(process.resizes).toEqual([{ cols: 120, rows: 40 }]);
		expect(createOptions.map((options) => options.shell)).toEqual([
			"C:\\Windows\\System32\\cmd.exe",
		]);

		process.emitExit({ exitCode: 0 });
		expect(exited).toEqual([{ id: "terminal-2", exitCode: 0 }]);
		expect(manager.get("terminal-2")).toBeNull();
	});

	test("kills all live terminal processes during shutdown", async () => {
		const createdProcesses = [
			new FakeTerminalProcess(),
			new FakeTerminalProcess(),
		];
		const processQueue = [...createdProcesses];
		const manager = new DesktopTerminalManager({
			createProcess: () => {
				const process = processQueue.shift();
				if (!process) throw new Error("expected a queued terminal process");
				return process;
			},
			defaultCwd: "/Users/tester",
			env: {},
			platform: "linux",
			idFactory: (() => {
				let next = 0;
				return () => `terminal-${++next}`;
			})(),
		});

		await manager.create({});
		await manager.create({});
		manager.closeAll();

		expect(createdProcesses.map((process) => process.kills)).toEqual([
			["SIGTERM"],
			["SIGTERM"],
		]);
	});
});
