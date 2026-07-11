import { describe, expect, it, vi } from "vitest";

import {
  buildCommands,
  commandsForScope,
  nativeShortcuts,
  VIEW_TARGETS,
  type ShortcutCtx,
} from "./registry";
import type { ViewActions } from "./useViewActions";

function fakeCtx(view: ViewActions = {}): ShortcutCtx {
  return {
    push: vi.fn(),
    openPalette: vi.fn(),
    toggleCheatSheet: vi.fn(),
    toggleTheme: vi.fn(),
    restartDaemon: vi.fn(),
    closeWindow: vi.fn(),
    openMailWindow: vi.fn(),
    openDumpsWindow: vi.fn(),
    openLinkSite: vi.fn(),
    parkFolder: vi.fn(),
    view: () => view,
  };
}

describe("VIEW_TARGETS", () => {
  it("covers the main views in sidebar order, About excluded", () => {
    expect(VIEW_TARGETS).toHaveLength(11);
    expect(VIEW_TARGETS[0]?.path).toBe("/overview");
    expect(VIEW_TARGETS[VIEW_TARGETS.length - 1]?.path).toBe("/doctor");
    expect(VIEW_TARGETS.map((v) => v.path)).not.toContain("/about");
    expect(VIEW_TARGETS.map((v) => v.path)).toContain("/integrations");
    expect(VIEW_TARGETS.map((v) => v.path)).toContain("/proxies");
  });

  it("binds exactly nine digit chords (⌘1…⌘9); Share is palette-only", () => {
    const withDigit = VIEW_TARGETS.filter((v) => v.digit !== undefined);
    expect(withDigit).toHaveLength(9);
    expect(withDigit.map((v) => v.digit)).toEqual([1, 2, 3, 4, 5, 6, 7, 8, 9]);
    expect(VIEW_TARGETS.find((v) => v.path === "/integrations")?.digit).toBeUndefined();
  });
});

describe("nativeShortcuts", () => {
  it("documents the macOS native window shortcuts", () => {
    const mac = nativeShortcuts(true);
    expect(mac.map((s) => s.title)).toEqual([
      "Minimise window",
      "Close window",
      "Quit Yerd",
    ]);
    expect(mac.every((s) => s.group === "Window")).toBe(true);
  });

  it("returns nothing on Linux (no native menu)", () => {
    expect(nativeShortcuts(false)).toEqual([]);
  });
});

describe("commandsForScope", () => {
  const all = buildCommands();

  it("surfaces ⌘1…⌘9 navigation only in the main window", () => {
    const main = commandsForScope(all, "main", false).filter((c) =>
      c.id.startsWith("nav:"),
    );
    expect(main).toHaveLength(11);
    expect(main.filter((c) => c.chord)).toHaveLength(9);
    const integrations = main.find((c) => c.id === "nav:/integrations");
    const proxies = main.find((c) => c.id === "nav:/proxies");
    expect(integrations).toBeDefined();
    expect(proxies).toBeDefined();
    expect(integrations?.chord).toBeUndefined();
    expect(proxies?.chord).toBeUndefined();
    expect(commandsForScope(all, "dumps", false).some((c) => c.id.startsWith("nav:"))).toBe(
      false,
    );
  });

  it("gives the dumps window its tab-cycle and find/refresh, not navigation", () => {
    const dumps = commandsForScope(all, "dumps", false).map((c) => c.id);
    expect(dumps).toContain("dumps-next-tab");
    expect(dumps).toContain("dumps-prev-tab");
    expect(dumps).toContain("find");
    expect(dumps).toContain("refresh");
    expect(dumps).not.toContain("new");
  });

  it("drops the Linux-only Close on macOS (the native menu owns Cmd+W)", () => {
    const macMain = commandsForScope(all, "main", true).map((c) => c.id);
    expect(macMain).not.toContain("close-window");
    const linuxMain = commandsForScope(all, "main", false).map((c) => c.id);
    expect(linuxMain).toContain("close-window");
  });

  it("does not bind a Quit chord (tray app; macOS quits via native menu)", () => {
    expect(all.some((c) => c.id === "quit")).toBe(false);
  });
});

describe("command run wiring", () => {
  const all = buildCommands();

  it("navigates to the matching path", () => {
    const ctx = fakeCtx();
    all.find((c) => c.id === "nav:/sites")?.run(ctx);
    expect(ctx.push).toHaveBeenCalledWith("/sites");
  });

  it("contextual commands no-op when the view registers no handler", () => {
    const ctx = fakeCtx({});
    const find = all.find((c) => c.id === "find");
    const create = all.find((c) => c.id === "new");
    expect(find).toBeDefined();
    expect(create).toBeDefined();
    expect(() => find?.run(ctx)).not.toThrow();
    expect(() => create?.run(ctx)).not.toThrow();
  });

  it("contextual commands call the active view handler", () => {
    const create = vi.fn();
    const ctx = fakeCtx({ create });
    all.find((c) => c.id === "new")?.run(ctx);
    expect(create).toHaveBeenCalledOnce();
  });

  it("Link Site / Park Folder route to the Sites dialogs via their chords", () => {
    const link = all.find((c) => c.id === "link-site");
    const park = all.find((c) => c.id === "park-folder");
    expect(link?.group).toBe("Sites");
    expect(park?.group).toBe("Sites");
    expect(link?.chord).toEqual({ mod: true, shift: true, key: "n" });
    expect(park?.chord).toEqual({ mod: true, shift: true, key: "p" });
    expect(link?.inPalette).toBe(true);

    const ctx = fakeCtx();
    link?.run(ctx);
    park?.run(ctx);
    expect(ctx.openLinkSite).toHaveBeenCalledOnce();
    expect(ctx.parkFolder).toHaveBeenCalledOnce();
  });

  it("opens the viewer windows via their chords", () => {
    const mail = all.find((c) => c.id === "open-mail");
    const dumps = all.find((c) => c.id === "open-dumps");
    expect(mail?.chord).toEqual({ mod: true, shift: true, key: "m" });
    expect(dumps?.chord).toEqual({ mod: true, shift: true, key: "d" });

    const ctx = fakeCtx();
    mail?.run(ctx);
    dumps?.run(ctx);
    expect(ctx.openMailWindow).toHaveBeenCalledOnce();
    expect(ctx.openDumpsWindow).toHaveBeenCalledOnce();
  });
});
