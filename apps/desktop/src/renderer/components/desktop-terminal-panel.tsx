import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { Button } from "@devo/ui/components/button";
import {
	Tooltip,
	TooltipContent,
	TooltipTrigger,
} from "@devo/ui/components/tooltip";
import { useAtomValue } from "jotai";
import { PanelBottomIcon, PlusIcon, XIcon } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { terminalNewTabNonceAtom } from "../atoms/terminal";
import { isElectron } from "../services/backend";

interface DesktopTerminalPanelProps {
	open: boolean;
	directory: string | null | undefined;
	onOpenChange: (open: boolean) => void;
}

type TerminalStatus = "idle" | "starting" | "running" | "exited" | "error";

interface TerminalTab {
	id: string;
	sessionId: string | null;
	cwd: string | null;
	status: TerminalStatus;
}

interface TerminalRuntime {
	terminal: Terminal;
	fitAddon: FitAddon;
	sessionStarting: boolean;
	cleanup: () => void;
}

const DEFAULT_TERMINAL_HEIGHT = 280;
const MIN_TERMINAL_HEIGHT = 180;
const MAX_TERMINAL_HEIGHT = 520;
const MIN_STABLE_TERMINAL_ROWS = 4;
const TERMINAL_FONT_SIZE = 13;

let nextTerminalTabId = 0;

function createTerminalTabId(): string {
	nextTerminalTabId += 1;
	return `desktop-terminal-tab-${nextTerminalTabId}`;
}

function folderNameLabel(path: string): string {
	const normalized = path.trim();
	if (!normalized) return "Terminal";
	if (/^[A-Za-z]:[\\/]*$/.test(normalized) || /^[\\/]+$/.test(normalized)) {
		return normalized;
	}
	const withoutTrailingSeparators = normalized.replace(/[\\/]+$/, "");
	const segments = withoutTrailingSeparators.split(/[\\/]+/).filter(Boolean);
	return segments.at(-1) ?? normalized;
}

function cssVariable(name: string, fallback: string): string {
	const value = getComputedStyle(document.documentElement)
		.getPropertyValue(name)
		.trim();
	return value || fallback;
}

function getTerminalTheme() {
	const isDark = document.documentElement.classList.contains("dark");
	const background = cssVariable("--background", "#ffffff");
	const foreground = cssVariable("--foreground", "#111827");
	const mutedForeground = cssVariable("--muted-foreground", "#6b7280");
	const ring = cssVariable("--ring", "#2563eb");
	const green = cssVariable("--chart-2", "#16a34a");
	const red = cssVariable("--destructive", "#dc2626");
	const yellow = isDark ? cssVariable("--chart-5", "#facc15") : "#8a6500";
	const magenta = cssVariable("--chart-4", isDark ? "#c084fc" : "#7e22ce");
	const cyan = cssVariable("--chart-1", isDark ? "#67e8f9" : "#007a99");

	return {
		background,
		foreground,
		cursor: foreground,
		cursorAccent: background,
		selectionBackground: isDark ? "#374151" : "#dbeafe",
		selectionInactiveBackground: isDark ? "#2a2a2a" : "#eef2ff",
		scrollbarSliderBackground: isDark ? "#5f5f5f66" : "#6b728066",
		scrollbarSliderHoverBackground: isDark ? "#8a8a8a80" : "#4b556380",
		scrollbarSliderActiveBackground: isDark ? "#afafaf99" : "#37415199",
		black: isDark ? "#6b7280" : foreground,
		blue: ring,
		brightBlack: mutedForeground,
		brightBlue: isDark ? "#93c5fd" : "#005f8d",
		brightCyan: isDark ? "#a5f3fc" : "#006f8a",
		brightGreen: isDark ? "#86efac" : "#047a36",
		brightMagenta: isDark ? "#d8b4fe" : "#7430c7",
		brightRed: isDark ? "#fca5a5" : red,
		brightWhite: foreground,
		brightYellow: isDark ? "#fde68a" : "#7a5a00",
		cyan,
		green,
		magenta,
		red,
		white: foreground,
		yellow,
	};
}

function getTerminalBridge() {
	if (!isElectron || !("terminal" in window.devo)) return null;
	return window.devo.terminal;
}

function statusLabel(status: TerminalStatus): string {
	if (status === "starting") return "Starting...";
	if (status === "error") return "Failed";
	if (status === "exited") return "Exited";
	return "";
}

function refreshTerminal(terminal: Terminal) {
	if (terminal.rows > 0) {
		terminal.refresh(0, terminal.rows - 1);
	}
}

export function DesktopTerminalPanel({
	open,
	directory,
	onOpenChange,
}: DesktopTerminalPanelProps) {
	const runtimesRef = useRef(new Map<string, TerminalRuntime>());
	const tabsRef = useRef<TerminalTab[]>([]);
	const activeTabIdRef = useRef<string | null>(null);
	const sessionToTabRef = useRef(new Map<string, string>());
	const [tabs, setTabs] = useState<TerminalTab[]>([]);
	const [activeTabId, setActiveTabId] = useState<string | null>(null);
	const [height, setHeight] = useState(DEFAULT_TERMINAL_HEIGHT);

	const setTabsState = useCallback((nextTabs: TerminalTab[]) => {
		tabsRef.current = nextTabs;
		setTabs(nextTabs);
	}, []);

	const setActiveTabState = useCallback((tabId: string | null) => {
		activeTabIdRef.current = tabId;
		setActiveTabId(tabId);
	}, []);

	const updateTab = useCallback(
		(tabId: string, patch: Partial<TerminalTab>) => {
			setTabsState(
				tabsRef.current.map((tab) =>
					tab.id === tabId ? { ...tab, ...patch } : tab,
				),
			);
		},
		[setTabsState],
	);

	const resizeTab = useCallback((tabId: string): boolean => {
		const terminalBridge = getTerminalBridge();
		const runtime = runtimesRef.current.get(tabId);
		const tab = tabsRef.current.find((candidate) => candidate.id === tabId);
		if (!terminalBridge || !runtime || !tab) return false;

		const dimensions = runtime.fitAddon.proposeDimensions();
		if (!dimensions || dimensions.rows < MIN_STABLE_TERMINAL_ROWS) {
			return false;
		}

		const changed =
			runtime.terminal.cols !== dimensions.cols ||
			runtime.terminal.rows !== dimensions.rows;
		if (changed) {
			runtime.terminal.resize(dimensions.cols, dimensions.rows);
			refreshTerminal(runtime.terminal);
			if (tab.sessionId) {
				terminalBridge.resize(tab.sessionId, dimensions.cols, dimensions.rows);
			}
		}
		return true;
	}, []);

	const startTabSession = useCallback(
		async (tabId: string) => {
			const terminalBridge = getTerminalBridge();
			const runtime = runtimesRef.current.get(tabId);
			const tab = tabsRef.current.find((candidate) => candidate.id === tabId);
			if (
				!terminalBridge ||
				!runtime ||
				!tab ||
				tab.sessionId ||
				runtime.sessionStarting ||
				runtime.terminal.rows < MIN_STABLE_TERMINAL_ROWS
			)
				return;

			runtime.sessionStarting = true;
			updateTab(tabId, { status: "starting" });
			try {
				const session = await terminalBridge.create({
					cwd: tab.cwd ?? undefined,
					cols: runtime.terminal.cols,
					rows: runtime.terminal.rows,
				});
				if (
					!runtimesRef.current.has(tabId) ||
					!tabsRef.current.some((candidate) => candidate.id === tabId)
				) {
					terminalBridge.close(session.id);
					return;
				}
				sessionToTabRef.current.set(session.id, tabId);
				updateTab(tabId, {
					sessionId: session.id,
					cwd: session.cwd,
					status: "running",
				});
			} catch (error) {
				updateTab(tabId, { status: "error" });
				runtime.terminal.write(
					`\r\nFailed to start terminal: ${String(error)}\r\n`,
				);
			} finally {
				runtime.sessionStarting = false;
			}
		},
		[updateTab],
	);

	const resizeAndStartTab = useCallback(
		(tabId: string) => {
			if (resizeTab(tabId)) void startTabSession(tabId);
		},
		[resizeTab, startTabSession],
	);

	const disposeTabRuntime = useCallback(
		(tabId: string, closeSession: boolean) => {
			const terminalBridge = getTerminalBridge();
			const runtime = runtimesRef.current.get(tabId);
			const tab = tabsRef.current.find((candidate) => candidate.id === tabId);
			if (closeSession && tab?.sessionId && terminalBridge) {
				terminalBridge.close(tab.sessionId);
			}
			if (tab?.sessionId) {
				sessionToTabRef.current.delete(tab.sessionId);
			}
			runtime?.cleanup();
			runtime?.terminal.dispose();
			runtimesRef.current.delete(tabId);
		},
		[],
	);

	const closeTab = useCallback(
		(tabId: string, options: { closeSession: boolean }) => {
			const previousTabs = tabsRef.current;
			const tabIndex = previousTabs.findIndex((tab) => tab.id === tabId);
			if (tabIndex === -1) return;

			disposeTabRuntime(tabId, options.closeSession);
			const nextTabs = previousTabs.filter((tab) => tab.id !== tabId);
			setTabsState(nextTabs);

			if (nextTabs.length === 0) {
				setActiveTabState(null);
				onOpenChange(false);
				return;
			}

			if (activeTabIdRef.current === tabId) {
				const nextActiveTab =
					nextTabs[Math.min(tabIndex, nextTabs.length - 1)] ?? nextTabs[0];
				setActiveTabState(nextActiveTab.id);
			}
		},
		[disposeTabRuntime, onOpenChange, setActiveTabState, setTabsState],
	);

	const closeAllTabs = useCallback(() => {
		for (const tab of tabsRef.current) {
			disposeTabRuntime(tab.id, true);
		}
		setTabsState([]);
		setActiveTabState(null);
	}, [disposeTabRuntime, setActiveTabState, setTabsState]);

	const closePanel = useCallback(() => {
		closeAllTabs();
		onOpenChange(false);
	}, [closeAllTabs, onOpenChange]);

	const addTerminalTab = useCallback(() => {
		const tab: TerminalTab = {
			id: createTerminalTabId(),
			sessionId: null,
			cwd: directory ?? null,
			status: "idle",
		};
		setTabsState([...tabsRef.current, tab]);
		setActiveTabState(tab.id);
	}, [directory, setActiveTabState, setTabsState]);

	const setTerminalContainerRef = useCallback(
		(tabId: string) => (container: HTMLDivElement | null) => {
			const terminalBridge = getTerminalBridge();
			if (!container || !terminalBridge || runtimesRef.current.has(tabId)) {
				return;
			}

			const terminal = new Terminal({
				cursorBlink: true,
				convertEol: true,
				fontFamily:
					'"IBM Plex Mono", "SFMono-Regular", Menlo, Monaco, Consolas, monospace',
				fontSize: TERMINAL_FONT_SIZE,
				lineHeight: 1.35,
				minimumContrastRatio: 4.5,
				scrollback: 5000,
				theme: getTerminalTheme(),
			});
			const fitAddon = new FitAddon();
			terminal.loadAddon(fitAddon);
			terminal.open(container);

			const themeObserver = new MutationObserver(() => {
				terminal.options.theme = getTerminalTheme();
			});
			themeObserver.observe(document.documentElement, {
				attributes: true,
				attributeFilter: ["class", "style"],
			});
			const resizeObserver = new ResizeObserver(() => {
				requestAnimationFrame(() => {
					if (activeTabIdRef.current === tabId) {
						resizeAndStartTab(tabId);
					}
				});
			});
			resizeObserver.observe(container);
			const inputDisposable = terminal.onData((data) => {
				const tab = tabsRef.current.find((candidate) => candidate.id === tabId);
				if (tab?.sessionId) terminalBridge.write(tab.sessionId, data);
			});

			runtimesRef.current.set(tabId, {
				terminal,
				fitAddon,
				sessionStarting: false,
				cleanup: () => {
					inputDisposable.dispose();
					resizeObserver.disconnect();
					themeObserver.disconnect();
				},
			});

			requestAnimationFrame(() => {
				terminal.options.theme = getTerminalTheme();
				if (activeTabIdRef.current === tabId) {
					resizeAndStartTab(tabId);
					terminal.focus();
				}
			});
		},
		[resizeAndStartTab],
	);

	useEffect(() => {
		const terminalBridge = getTerminalBridge();
		if (!terminalBridge) return;

		const unsubscribeData = terminalBridge.onData(({ id, data }) => {
			const tabId = sessionToTabRef.current.get(id);
			if (!tabId) return;
			const runtime = runtimesRef.current.get(tabId);
			runtime?.terminal.write(data, () => {
				runtime.terminal.scrollToBottom();
				if (activeTabIdRef.current === tabId) {
					refreshTerminal(runtime.terminal);
				}
			});
		});
		const unsubscribeExit = terminalBridge.onExit(({ id }) => {
			const tabId = sessionToTabRef.current.get(id);
			if (!tabId) return;
			closeTab(tabId, { closeSession: false });
		});

		return () => {
			unsubscribeData();
			unsubscribeExit();
		};
	}, [closeTab]);

	useEffect(() => {
		if (!open || !getTerminalBridge() || tabsRef.current.length > 0) return;
		addTerminalTab();
	}, [open, addTerminalTab]);

	const newTabNonce = useAtomValue(terminalNewTabNonceAtom);
	const previousNewTabNonceRef = useRef(newTabNonce);
	useEffect(() => {
		if (newTabNonce === previousNewTabNonceRef.current) return;
		previousNewTabNonceRef.current = newTabNonce;
		if (open) addTerminalTab();
	}, [addTerminalTab, newTabNonce, open]);

	useEffect(() => {
		if (!open || !activeTabId || height <= 0) return;
		requestAnimationFrame(() => {
			resizeAndStartTab(activeTabId);
			const runtime = runtimesRef.current.get(activeTabId);
			if (runtime) {
				refreshTerminal(runtime.terminal);
				runtime.terminal.focus();
			}
		});
	}, [activeTabId, height, open, resizeAndStartTab]);

	useEffect(() => {
		if (!open) return;
		const resizeActiveTab = () => {
			const tabId = activeTabIdRef.current;
			if (tabId) resizeAndStartTab(tabId);
		};
		window.addEventListener("resize", resizeActiveTab);
		return () => window.removeEventListener("resize", resizeActiveTab);
	}, [open, resizeAndStartTab]);

	useEffect(() => {
		return () => closeAllTabs();
	}, [closeAllTabs]);

	const handleResizePointerDown = useCallback(
		(event: React.PointerEvent<HTMLDivElement>) => {
			event.preventDefault();
			const startY = event.clientY;
			const startHeight = height;
			const handlePointerMove = (moveEvent: PointerEvent) => {
				const nextHeight = Math.min(
					MAX_TERMINAL_HEIGHT,
					Math.max(
						MIN_TERMINAL_HEIGHT,
						startHeight + startY - moveEvent.clientY,
					),
				);
				setHeight(nextHeight);
			};
			const handlePointerUp = () => {
				window.removeEventListener("pointermove", handlePointerMove);
				window.removeEventListener("pointerup", handlePointerUp);
			};

			window.addEventListener("pointermove", handlePointerMove);
			window.addEventListener("pointerup", handlePointerUp);
		},
		[height],
	);

	const activeTab =
		tabs.find((tab) => tab.id === activeTabId) ?? tabs[0] ?? null;
	const activeStatusLabel = activeTab ? statusLabel(activeTab.status) : "";

	if (!getTerminalBridge()) return null;

	return (
		<section
			data-slot="desktop-terminal-panel"
			data-state={open ? "open" : "closed"}
			className="flex shrink-0 flex-col overflow-hidden border-t border-border bg-background transition-[height] duration-150"
			style={{ height: open ? height : 0 }}
			aria-hidden={!open}
		>
			<div
				className="h-1 cursor-ns-resize bg-transparent hover:bg-primary/20"
				onPointerDown={handleResizePointerDown}
			/>
			<div className="flex h-10 shrink-0 items-center justify-between border-b border-border/70 px-3">
				<div className="flex min-w-0 items-center gap-1">
					<div className="flex min-w-0 items-center gap-1 overflow-x-auto">
						{tabs.map((tab) => {
							const isActive = tab.id === activeTabId;
							const terminalTitle = tab.cwd ?? directory ?? "Terminal";
							const folderName = folderNameLabel(terminalTitle);
							return (
								<div
									key={tab.id}
									className={`group flex h-7 min-w-0 max-w-[22rem] shrink-0 items-center gap-1 rounded-md px-2 transition-colors ${
										isActive
											? "bg-muted text-foreground"
											: "text-muted-foreground hover:bg-muted/70 hover:text-foreground"
									}`}
								>
									<button
										type="button"
										className="flex min-w-0 items-center gap-2 focus-visible:outline-none"
										style={{ fontSize: TERMINAL_FONT_SIZE }}
										title={terminalTitle}
										onClick={() => setActiveTabState(tab.id)}
									>
										<PanelBottomIcon
											className="shrink-0 text-muted-foreground"
											style={{
												height: TERMINAL_FONT_SIZE,
												width: TERMINAL_FONT_SIZE,
											}}
											aria-hidden="true"
										/>
										<span className="truncate font-medium">
											{folderName}
										</span>
									</button>
									<button
										type="button"
										aria-label="Close"
										className="grid size-4 shrink-0 place-items-center rounded-sm text-muted-foreground opacity-0 transition group-hover:opacity-100 hover:bg-background/80 hover:text-foreground focus-visible:opacity-100 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
										onClick={(event) => {
											event.stopPropagation();
											closeTab(tab.id, { closeSession: true });
										}}
									>
										<XIcon className="size-3" aria-hidden="true" />
									</button>
								</div>
							);
						})}
					</div>
					<Tooltip>
						<TooltipTrigger
							aria-label="New terminal"
							render={
								<Button
									type="button"
									variant="ghost"
									size="icon"
									className="size-7 shrink-0 rounded-md hover:bg-muted hover:text-foreground"
									onClick={addTerminalTab}
								/>
							}
						>
							<PlusIcon className="size-3.5" aria-hidden="true" />
						</TooltipTrigger>
						<TooltipContent>New terminal</TooltipContent>
					</Tooltip>
					{activeStatusLabel && (
						<span className="shrink-0 pl-1 text-xs text-muted-foreground">
							{activeStatusLabel}
						</span>
					)}
				</div>
				<div className="flex items-center gap-1">
					<Tooltip>
						<TooltipTrigger
							aria-label="Close"
							render={
								<Button
									type="button"
									variant="ghost"
									size="icon"
									className="size-7"
									onClick={closePanel}
								/>
							}
						>
							<XIcon className="size-3.5" aria-hidden="true" />
						</TooltipTrigger>
						<TooltipContent>Close</TooltipContent>
					</Tooltip>
				</div>
			</div>
			<div className="box-border min-h-0 flex-1 overflow-hidden bg-background px-3 pt-2 pb-4">
				<div className="relative h-full min-h-0 min-w-0 overflow-hidden [&_.xterm]:h-full [&_.xterm-scrollable-element]:!bg-transparent [&_.xterm-viewport]:!bg-background">
					{tabs.map((tab) => (
						<div
							key={tab.id}
							ref={setTerminalContainerRef(tab.id)}
							className={`absolute inset-0 overflow-hidden ${
								tab.id === activeTabId ? "block" : "hidden"
							}`}
						/>
					))}
				</div>
			</div>
		</section>
	);
}
