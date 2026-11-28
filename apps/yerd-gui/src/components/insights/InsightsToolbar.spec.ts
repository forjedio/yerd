import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import InsightsToolbar from "./InsightsToolbar.vue";
import type { SiteEntry } from "@/ipc/types";

const SITES: SiteEntry[] = [
  { name: "acme", document_root: "/srv/acme", php: "8.5", secure: true, kind: "linked", framework: "laravel" },
];

function render(recordingSites: string[], wide = true) {
  return mount(InsightsToolbar, {
    props: {
      sites: SITES,
      site: "acme",
      tld: "test",
      recordingSites,
      statusLoaded: true,
      range: "1h" as const,
      paused: false,
      busy: null,
      wide,
    },
  });
}

describe("InsightsToolbar", () => {
  it("shows the site with its framework and PHP version", () => {
    const text = render(["acme"]).get('[data-testid="site-picker"]').text();
    expect(text).toContain("acme.test");
    expect(text).toContain("Laravel · PHP 8.5");
  });

  it("toggles recording from the pill", async () => {
    const on = render(["acme"]);
    expect(on.get('[data-testid="recording-pill"]').text()).toBe("Recording · this site");
    await on.get('[data-testid="recording-pill"]').trigger("click");
    expect(on.emitted("toggleRecording")?.[0]).toEqual([false]);
    const off = render([]);
    await off.get('[data-testid="recording-pill"]').trigger("click");
    expect(off.emitted("toggleRecording")?.[0]).toEqual([true]);
  });

  it("offers only the three ranges", () => {
    const labels = render(["acme"]).findAll('[role="radio"]').map((r) => r.text());
    expect(labels).toEqual(["5m", "1h", "24h"]);
  });

  it("emits pause, clear and settings", async () => {
    const w = render(["acme"], false);
    await w.get('[data-testid="pause"]').trigger("click");
    await w.get('[data-testid="clear"]').trigger("click");
    await w.get('[data-testid="settings"]').trigger("click");
    expect(w.emitted("togglePause")).toHaveLength(1);
    expect(w.emitted("clear")).toHaveLength(1);
    expect(w.emitted("settings")).toHaveLength(1);
    expect(w.get('[data-testid="recording-pill"]').text()).toBe("Recording");
  });
});
