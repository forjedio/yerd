import { describe, expect, it, vi } from "vitest";

import { buildSiteCommands, type SiteCommandHandlers } from "./useSiteCommands";
import type { ShortcutCtx } from "./registry";
import type { Site } from "@/ipc/types";

function site(name: string, secure: boolean): Site {
  return { name, document_root: `/srv/${name}`, php: "8.4", secure, kind: "linked" };
}

const noop: SiteCommandHandlers = { onOpen: vi.fn(), onToggleSecure: vi.fn() };
const ctx = {} as ShortcutCtx;

describe("buildSiteCommands", () => {
  it("yields two commands per site, ordered by name descending", () => {
    const cmds = buildSiteCommands([site("alpha", false), site("zeta", true)], "test", noop);
    expect(cmds.map((c) => c.id)).toEqual([
      "site-open:zeta",
      "site-secure:zeta",
      "site-open:alpha",
      "site-secure:alpha",
    ]);
  });

  it("labels open and secure/unsecure by state, grouped by domain, no chord", () => {
    const [open, secure] = buildSiteCommands([site("alpha", false)], "test", noop);
    expect(open.title).toBe("Open alpha.test");
    expect(open.group).toBe("alpha.test");
    expect(open.chord).toBeUndefined();
    expect(open.inPalette).toBe(true);
    expect(secure.title).toBe("Secure alpha.test");

    const [, unsecure] = buildSiteCommands([site("beta", true)], "test", noop);
    expect(unsecure.title).toBe("Unsecure beta.test");
  });

  it("uses the given tld in titles and groups", () => {
    const [open] = buildSiteCommands([site("alpha", false)], "dev", noop);
    expect(open.title).toBe("Open alpha.dev");
    expect(open.group).toBe("alpha.dev");
  });

  it("delegates run to the injected handlers", () => {
    const onOpen = vi.fn();
    const onToggleSecure = vi.fn();
    const s = site("alpha", false);
    const [open, secure] = buildSiteCommands([s], "test", { onOpen, onToggleSecure });
    open.run(ctx);
    secure.run(ctx);
    expect(onOpen).toHaveBeenCalledWith(s);
    expect(onToggleSecure).toHaveBeenCalledWith(s);
  });
});
