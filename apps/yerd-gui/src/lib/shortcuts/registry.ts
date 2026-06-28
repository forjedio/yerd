/**
 * The command catalog: the single source of truth for the keyboard dispatcher,
 * the command palette, and the cheat-sheet. Commands reference only the injected
 * `ShortcutCtx`, never app singletons directly, so the registry stays free of
 * Tauri/IPC imports and is unit-testable with a fake context.
 *
 * `scopes` lists the windows a command is active in ("main" is the app shell;
 * "dumps"/"mails" are the standalone viewer windows). `linuxOnly` commands are
 * skipped on macOS, where the native app menu already owns them (Close/Quit).
 */
import type { Chord } from "./chord";
import type { ViewActions } from "./useViewActions";

export type WindowScope = "main" | "dumps" | "mails";

const ALL: WindowScope[] = ["main", "dumps", "mails"];

/** Everything a command needs to act, injected by the dispatcher. */
export interface ShortcutCtx {
  /** Navigate the main window's router. */
  push: (path: string) => void;
  openPalette: () => void;
  openCheatSheet: () => void;
  toggleTheme: () => void;
  restartDaemon: () => void;
  closeWindow: () => void;
  /** Live contextual handlers for the currently mounted view. */
  view: () => ViewActions;
}

export interface Command {
  id: string;
  title: string;
  group: string;
  chord: Chord;
  scopes: WindowScope[];
  /** Skipped on macOS (the native menu provides it there). */
  linuxOnly?: boolean;
  /** Listed in the command palette (navigation + global actions). */
  inPalette?: boolean;
  run: (ctx: ShortcutCtx) => void;
}

/** Main-window views reachable by ⌘1…⌘9, in sidebar order (About omitted). */
export const VIEW_TARGETS: { path: string; title: string }[] = [
  { path: "/overview", title: "Overview" },
  { path: "/php", title: "PHP" },
  { path: "/sites", title: "Sites" },
  { path: "/tooling", title: "Tooling" },
  { path: "/services", title: "Services" },
  { path: "/mail", title: "Mail" },
  { path: "/dumps", title: "Dumps" },
  { path: "/general", title: "Settings" },
  { path: "/doctor", title: "Doctor" },
];

/** Build the full command catalog. Pure: no side effects until a `run` fires. */
export function buildCommands(): Command[] {
  const nav: Command[] = VIEW_TARGETS.map((v, i) => ({
    id: `nav:${v.path}`,
    title: `Go to ${v.title}`,
    group: "Go to",
    chord: { mod: true, code: `Digit${i + 1}` },
    scopes: ["main"],
    inPalette: true,
    run: (ctx) => ctx.push(v.path),
  }));

  const rest: Command[] = [
    {
      id: "palette",
      title: "Command palette",
      group: "General",
      chord: { mod: true, key: "k" },
      scopes: ["main"],
      run: (ctx) => ctx.openPalette(),
    },
    {
      id: "cheatsheet",
      title: "Keyboard shortcuts",
      group: "General",
      chord: { mod: true, key: "/" },
      scopes: ["main"],
      inPalette: true,
      run: (ctx) => ctx.openCheatSheet(),
    },
    {
      id: "settings",
      title: "Open Settings",
      group: "General",
      chord: { mod: true, key: "," },
      scopes: ["main"],
      inPalette: true,
      run: (ctx) => ctx.push("/general"),
    },
    {
      id: "restart-daemon",
      title: "Restart daemon",
      group: "Actions",
      chord: { mod: true, shift: true, key: "r" },
      scopes: ["main"],
      inPalette: true,
      run: (ctx) => ctx.restartDaemon(),
    },
    {
      id: "toggle-theme",
      title: "Toggle light / dark theme",
      group: "Actions",
      chord: { mod: true, shift: true, key: "l" },
      scopes: ALL,
      inPalette: true,
      run: (ctx) => ctx.toggleTheme(),
    },
    {
      id: "find",
      title: "Find in view",
      group: "Actions",
      chord: { mod: true, key: "f" },
      scopes: ["main", "dumps"],
      run: (ctx) => ctx.view().find?.(),
    },
    {
      id: "new",
      title: "New / Add",
      group: "Actions",
      chord: { mod: true, key: "n" },
      scopes: ["main"],
      run: (ctx) => ctx.view().create?.(),
    },
    {
      id: "refresh",
      title: "Refresh view",
      group: "Actions",
      chord: { mod: true, key: "r" },
      scopes: ALL,
      run: (ctx) => ctx.view().refresh?.(),
    },
    {
      id: "dumps-prev-tab",
      title: "Previous tab",
      group: "View",
      chord: { ctrl: true, shift: true, code: "Tab" },
      scopes: ["dumps"],
      run: (ctx) => ctx.view().prevTab?.(),
    },
    {
      id: "dumps-next-tab",
      title: "Next tab",
      group: "View",
      chord: { ctrl: true, code: "Tab" },
      scopes: ["dumps"],
      run: (ctx) => ctx.view().nextTab?.(),
    },
    {
      id: "close-window",
      title: "Close window",
      group: "Window",
      chord: { mod: true, key: "w" },
      scopes: ALL,
      linuxOnly: true,
      run: (ctx) => ctx.closeWindow(),
    },
    {
      id: "quit",
      title: "Quit",
      group: "Window",
      chord: { mod: true, key: "q" },
      scopes: ["main"],
      linuxOnly: true,
      run: (ctx) => ctx.closeWindow(),
    },
  ];

  return [...nav, ...rest];
}

/** Commands active in `scope` on this platform (drops macOS-native duplicates). */
export function commandsForScope(
  commands: Command[],
  scope: WindowScope,
  isMac: boolean,
): Command[] {
  return commands.filter(
    (c) => c.scopes.includes(scope) && !(c.linuxOnly && isMac),
  );
}
