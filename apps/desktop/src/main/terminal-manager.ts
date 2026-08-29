import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { waitForEnv } from "./shell-env";

export interface TerminalSessionInfo {
	id: string;
	cwd: string;
	shell: string;
	cols: number;
	rows: number;
}

export interface TerminalCreateOptions {
	cwd?: string;
	cols?: number;
	rows?: number;
}

export interface TerminalCreateProcessOptions {
	shell: string;
	args: string[];
	cwd: string;
	env: Record<string, string>;
	cols: number;
	rows: number;
}

export interface TerminalProcess {
	write(data: string): void;
	resize(cols: number, rows: number): void;
	kill(signal?: string): void;
	onData(callback: (data: string) => void): void;
	onExit(
		callback: (event: { exitCode: number; signal?: number }) => void,
	): void;
}

type TerminalProcessFactory = (
	options: TerminalCreateProcessOptions,
) => TerminalProcess | Promise<TerminalProcess>;

type FileExists = (path: string) => boolean;
type FileReader = (path: string) => string;

interface DesktopTerminalManagerOptions {
	createProcess?: TerminalProcessFactory;
	defaultCwd?: string;
	env?: NodeJS.ProcessEnv | Record<string, string | undefined>;
	platform?: NodeJS.Platform;
	idFactory?: () => string;
	exists?: FileExists;
	readFile?: FileReader;
}

interface TerminalSession {
	info: TerminalSessionInfo;
	process: TerminalProcess;
}

interface ShellCommand {
	shell: string;
	args: string[];
}

interface ShellResolutionDeps {
	exists: FileExists;
	readFile: FileReader;
}

function createTerminalId(): string {
	return `terminal-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

function normalizeEnv(
	env: NodeJS.ProcessEnv | Record<string, string | undefined>,
): Record<string, string> {
	const normalized: Record<string, string> = {};
	for (const [key, value] of Object.entries(env)) {
		if (value !== undefined) normalized[key] = value;
	}
	return normalized;
}

function getEnvValue(env: Record<string, string>, key: string): string | undefined {
	const exact = env[key];
	if (exact !== undefined) return exact;
	const normalizedKey = key.toLowerCase();
	for (const [name, value] of Object.entries(env)) {
		if (name.toLowerCase() === normalizedKey) return value;
	}
	return undefined;
}

function joinWindowsPath(base: string, ...segments: string[]): string {
	let path = base.replace(/[\\/]$/, "");
	for (const segment of segments) {
		path += `\\${segment}`;
	}
	return path;
}

function windowsTerminalSettingsPaths(env: Record<string, string>): string[] {
	const localAppData = getEnvValue(env, "LOCALAPPDATA");
	if (!localAppData) return [];
	return [
		joinWindowsPath(
			localAppData,
			"Packages",
			"Microsoft.WindowsTerminal_8wekyb3d8bbwe",
			"LocalState",
			"settings.json",
		),
		joinWindowsPath(
			localAppData,
			"Packages",
			"Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe",
			"LocalState",
			"settings.json",
		),
	];
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === "object" && value !== null && !Array.isArray(value);
}

function readWindowsTerminalSettings(
	env: Record<string, string>,
	deps: ShellResolutionDeps,
): Record<string, unknown> | null {
	for (const settingsPath of windowsTerminalSettingsPaths(env)) {
		if (!deps.exists(settingsPath)) continue;
		try {
			const parsed = JSON.parse(deps.readFile(settingsPath));
			if (isRecord(parsed)) return parsed;
		} catch {
			// Ignore malformed profile settings and keep looking for a usable shell.
		}
	}
	return null;
}

function windowsTerminalProfiles(
	settings: Record<string, unknown>,
): Record<string, unknown>[] {
	const profiles = settings.profiles;
	if (!isRecord(profiles) || !Array.isArray(profiles.list)) return [];
	return profiles.list.filter(isRecord);
}

function expandWindowsEnvVars(
	value: string,
	env: Record<string, string>,
): string {
	return value.replace(/%([^%]+)%/g, (match, key: string) => {
		return getEnvValue(env, key) ?? match;
	});
}

function splitWindowsCommandLine(commandLine: string): string[] {
	const parts: string[] = [];
	let current = "";
	let inQuotes = false;

	for (const char of commandLine.trim()) {
		if (char === '"') {
			inQuotes = !inQuotes;
			continue;
		}
		if (/\s/.test(char) && !inQuotes) {
			if (current) {
				parts.push(current);
				current = "";
			}
			continue;
		}
		current += char;
	}

	if (current) parts.push(current);
	return parts;
}

function parseWindowsCommandLine(
	commandLine: string,
	env: Record<string, string>,
): ShellCommand | null {
	const parts = splitWindowsCommandLine(expandWindowsEnvVars(commandLine, env));
	if (parts.length === 0) return null;

	let executableParts = [parts[0]];
	let args = parts.slice(1);
	if (!parts[0].toLowerCase().endsWith(".exe")) {
		const executableEnd = parts.findIndex((part, index) => {
			return index > 0 && part.toLowerCase().endsWith(".exe");
		});
		if (executableEnd !== -1) {
			executableParts = parts.slice(0, executableEnd + 1);
			args = parts.slice(executableEnd + 1);
		}
	}

	return { shell: executableParts.join(" "), args };
}

function findExecutableOnPath(
	executable: string,
	env: Record<string, string>,
	deps: ShellResolutionDeps,
): string | null {
	const pathValue = getEnvValue(env, "PATH");
	if (!pathValue) return null;
	const hasExtension = /\.[^\\/]+$/.test(executable);
	const pathExtensions = (getEnvValue(env, "PATHEXT") ?? ".COM;.EXE;.BAT;.CMD")
		.split(";")
		.filter(Boolean);
	const names = hasExtension
		? [executable]
		: [executable, ...pathExtensions.map((extension) => `${executable}${extension}`)];

	for (const directory of pathValue.split(";")) {
		const normalizedDirectory = directory.trim().replace(/^"|"$/g, "");
		if (!normalizedDirectory) continue;
		for (const name of names) {
			const candidate = joinWindowsPath(normalizedDirectory, name);
			if (deps.exists(candidate)) return candidate;
		}
	}

	return null;
}

function isPowerShellExecutable(shell: string): boolean {
	const base = shell.replace(/^.*[\\/]/, "").toLowerCase();
	return base === "pwsh.exe" || base === "powershell.exe" || base === "pwsh" || base === "powershell";
}

function withPowerShellNoLogo(command: ShellCommand): ShellCommand {
	if (!isPowerShellExecutable(command.shell)) return command;
	const hasNoLogo = command.args.some((arg) => arg.toLowerCase() === "-nologo");
	if (hasNoLogo) return command;
	return { shell: command.shell, args: ["-NoLogo", ...command.args] };
}

function resolveWindowsTerminalSourceShell(
	source: string,
	env: Record<string, string>,
	deps: ShellResolutionDeps,
): ShellCommand | null {
	if (source === "Windows.Terminal.PowershellCore") {
		return {
			shell: findExecutableOnPath("pwsh.exe", env, deps) ?? "pwsh.exe",
			args: ["-NoLogo"],
		};
	}
	if (source === "Windows.Terminal.WindowsPowerShell") {
		return {
			shell:
				findExecutableOnPath("powershell.exe", env, deps) ?? "powershell.exe",
			args: ["-NoLogo"],
		};
	}
	if (source === "Windows.Terminal.CommandPrompt") {
		return {
			shell: getEnvValue(env, "ComSpec") ?? "cmd.exe",
			args: [],
		};
	}
	return null;
}

function resolveWindowsTerminalShell(
	env: Record<string, string>,
	deps: ShellResolutionDeps,
): ShellCommand | null {
	const settings = readWindowsTerminalSettings(env, deps);
	const defaultProfile =
		typeof settings?.defaultProfile === "string"
			? settings.defaultProfile.toLowerCase()
			: null;
	if (!settings || !defaultProfile) return null;

	const profile = windowsTerminalProfiles(settings).find((candidate) => {
		const guid =
			typeof candidate.guid === "string" ? candidate.guid.toLowerCase() : null;
		const name =
			typeof candidate.name === "string" ? candidate.name.toLowerCase() : null;
		return guid === defaultProfile || name === defaultProfile;
	});
	if (!profile) return null;

	if (typeof profile.commandline === "string") {
		const command = parseWindowsCommandLine(profile.commandline, env);
		if (command) return command;
	}
	if (typeof profile.source === "string") {
		return resolveWindowsTerminalSourceShell(profile.source, env, deps);
	}
	return null;
}

function resolveWindowsShellCommand(
	env: Record<string, string>,
	deps: ShellResolutionDeps,
): ShellCommand {
	const windowsTerminalShell = resolveWindowsTerminalShell(env, deps);
	if (windowsTerminalShell) return withPowerShellNoLogo(windowsTerminalShell);

	const configuredShell = getEnvValue(env, "SHELL");
	if (configuredShell) {
		return withPowerShellNoLogo({ shell: configuredShell, args: [] });
	}

	const pwsh = findExecutableOnPath("pwsh.exe", env, deps);
	if (pwsh) return { shell: pwsh, args: ["-NoLogo"] };

	const powershell = findExecutableOnPath("powershell.exe", env, deps);
	if (powershell) return { shell: powershell, args: ["-NoLogo"] };

	return {
		shell:
			getEnvValue(env, "ComSpec") ?? "C:\\Windows\\System32\\cmd.exe",
		args: [],
	};
}

function resolveShellCommand(
	platform: NodeJS.Platform,
	env: Record<string, string>,
	deps: ShellResolutionDeps,
): ShellCommand {
	if (platform === "win32") {
		return resolveWindowsShellCommand(env, deps);
	}
	return {
		shell: env.SHELL ?? (platform === "darwin" ? "/bin/zsh" : "/bin/bash"),
		args: ["-l"],
	};
}

async function createNodePtyProcess(
	options: TerminalCreateProcessOptions,
): Promise<TerminalProcess> {
	const pty = await import("node-pty");
	const process = pty.spawn(options.shell, options.args, {
		name: "xterm-256color",
		cols: options.cols,
		rows: options.rows,
		cwd: options.cwd,
		env: options.env,
	});

	return {
		write: (data) => process.write(data),
		resize: (cols, rows) => process.resize(cols, rows),
		kill: (signal) => process.kill(signal),
		onData: (callback) => {
			process.onData(callback);
		},
		onExit: (callback) => {
			process.onExit(callback);
		},
	};
}

export class DesktopTerminalManager {
	private readonly createProcess: TerminalProcessFactory;
	private readonly defaultCwd: string;
	private readonly env: NodeJS.ProcessEnv | Record<string, string | undefined>;
	private readonly platform: NodeJS.Platform;
	private readonly idFactory: () => string;
	private readonly shellDeps: ShellResolutionDeps;
	private readonly sessions = new Map<string, TerminalSession>();
	private readonly dataListeners = new Set<
		(id: string, data: string) => void
	>();
	private readonly exitListeners = new Set<
		(id: string, event: { exitCode: number; signal?: number }) => void
	>();

	constructor(options: DesktopTerminalManagerOptions = {}) {
		this.createProcess = options.createProcess ?? createNodePtyProcess;
		this.defaultCwd = options.defaultCwd ?? homedir();
		this.env = options.env ?? process.env;
		this.platform = options.platform ?? process.platform;
		this.idFactory = options.idFactory ?? createTerminalId;
		this.shellDeps = {
			exists: options.exists ?? existsSync,
			readFile:
				options.readFile ??
				((path) => {
					return readFileSync(path, "utf-8");
				}),
		};
	}

	onData(listener: (id: string, data: string) => void): () => void {
		this.dataListeners.add(listener);
		return () => this.dataListeners.delete(listener);
	}

	onExit(
		listener: (
			id: string,
			event: { exitCode: number; signal?: number },
		) => void,
	): () => void {
		this.exitListeners.add(listener);
		return () => this.exitListeners.delete(listener);
	}

	async create(options: TerminalCreateOptions): Promise<TerminalSessionInfo> {
		await waitForEnv();
		const env: Record<string, string> = {
			...normalizeEnv(this.env),
			DISABLE_AUTO_UPDATE: "true",
			// Suppress PowerShell 7's startup update banner (dark ANSI block in light themes).
			POWERSHELL_UPDATECHECK: "Off",
			TERM: "xterm-256color",
			COLORTERM: "truecolor",
		};
		delete env.ZSH_TMUX_AUTOSTARTED;
		delete env.ZSH_TMUX_AUTOSTART;
		const cols = options.cols ?? 80;
		const rows = options.rows ?? 24;
		const cwd = options.cwd || this.defaultCwd;
		const shellCommand = resolveShellCommand(this.platform, env, this.shellDeps);
		const id = this.idFactory();
		const terminalProcess = await this.createProcess({
			shell: shellCommand.shell,
			args: shellCommand.args,
			cwd,
			env,
			cols,
			rows,
		});
		const info = { id, cwd, shell: shellCommand.shell, cols, rows };

		terminalProcess.onData((data) => {
			for (const listener of this.dataListeners) {
				listener(id, data);
			}
		});
		terminalProcess.onExit((event) => {
			this.sessions.delete(id);
			for (const listener of this.exitListeners) {
				listener(id, event);
			}
		});

		this.sessions.set(id, { info, process: terminalProcess });
		return info;
	}

	get(id: string): TerminalSessionInfo | null {
		return this.sessions.get(id)?.info ?? null;
	}

	write(id: string, data: string): void {
		this.sessions.get(id)?.process.write(data);
	}

	resize(id: string, cols: number, rows: number): void {
		const session = this.sessions.get(id);
		if (!session) return;
		session.info = { ...session.info, cols, rows };
		session.process.resize(cols, rows);
	}

	close(id: string): void {
		const session = this.sessions.get(id);
		if (!session) return;
		this.sessions.delete(id);
		session.process.kill();
	}

	closeAll(): void {
		for (const id of [...this.sessions.keys()]) {
			this.close(id);
		}
	}
}

export const desktopTerminalManager = new DesktopTerminalManager();
