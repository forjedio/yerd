import { describe, expect, it, vi } from "vitest";

import {
  buildCommands,
  commandsForScope,
  VIEW_TARGETS,
  type ShortcutCtx,
} from "./registry";
import type { ViewActions } from "./useViewActions";

function fakeCtx(view: ViewActions = {}): ShortcutCtx {
  return {
    push: vi.fn(),
    openPalette: vi.fn(),
    openCheatSheet: vi.fn(),
    toggleTheme: vi.fn(),
    restartDaemon: vi.fn(),
    closeWindow: vi.fn(),
    view: () => view,
  };
}

describe("VIEW_TARGETS", () => {
  it("covers nine views in sidebar order, About excluded", () => {
    expect(VIEW_TARGETS).toHaveLength(9);
    expect(VIEW_TARGETS[0]?.path).toBe("/overview");
    expect(VIEW_TARGETS[VIEW_TARGETS.length - 1]?.path).toBe("/doctor");
    expect(VIEW_TARGETS.map((v) => v.path)).not.toContain("/about");
  });
});

describe("commandsForScope", () => {
  const all = buildCommands();

  it("surfaces ⌘1…⌘9 navigation only in the main window", () => {
    const main = commandsForScope(all, "main", false).filter((c) =>
      c.id.startsWith("nav:"),
    );
    expect(main).toHaveLength(9);
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

  it("drops Close/Quit on macOS (the native menu owns them)", () => {
    const macMain = commandsForScope(all, "main", true).map((c) => c.id);
    expect(macMain).not.toContain("close-window");
    expect(macMain).not.toContain("quit");
    const linuxMain = commandsForScope(all, "main", false).map((c) => c.id);
    expect(linuxMain).toContain("close-window");
    expect(linuxMain).toContain("quit");
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
    expect(() => all.find((c) => c.id === "find")?.run(ctx)).not.toThrow();
    expect(() => all.find((c) => c.id === "new")?.run(ctx)).not.toThrow();
  });

  it("contextual commands call the active view handler", () => {
    const create = vi.fn();
    const ctx = fakeCtx({ create });
    all.find((c) => c.id === "new")?.run(ctx);
    expect(create).toHaveBeenCalledOnce();
  });
});
