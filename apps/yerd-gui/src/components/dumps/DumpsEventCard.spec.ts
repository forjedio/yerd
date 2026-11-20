import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { defineComponent, h } from "vue";

const openPath = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(), openPath: (p: string) => openPath(p) }));

import DumpsEventCard from "./DumpsEventCard.vue";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useToast } from "@/composables/useToast";
import type { DumpCategory, DumpEvent } from "@/ipc/types";

function ev(category: DumpCategory, payload: Record<string, unknown>): DumpEvent {
  return { id: 1, category, ts_ms: 0, site: "codestash.test", request_id: "r", payload };
}

const mounted: VueWrapper[] = [];

function mountCard(event: DumpEvent) {
  const Host = defineComponent({
    setup: () => () => h(TooltipProvider, null, () => h(DumpsEventCard, { event })),
  });
  const wrapper = mount(Host, { attachTo: document.body });
  mounted.push(wrapper);
  return wrapper;
}

function tags(w: VueWrapper): string[] {
  return w.findAll('[data-testid="dump-tag"]').map((t) => t.text());
}

beforeEach(() => {
  openPath.mockReset();
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("DumpsEventCard", () => {
  it.each([
    ["query", { sql: "select 1", time_ms: 1.824, row_count: 1 }, ["QUERY", "1.82 ms", "1 row"]],
    ["http", { method: "get", status: 200, duration_ms: 142.1, url: "https://x" }, ["HTTP", "GET", "200", "142.10 ms"]],
    ["cache", { event: "hit", key: "k" }, ["CACHE", "HIT"]],
  ] as const)("tags a %s event in drawn order", (category, payload, expected) => {
    expect(tags(mountCard(ev(category, payload)))).toEqual(expected);
  });

  it("opens an absolute caller and toasts the relative path and reason on failure", async () => {
    openPath.mockRejectedValueOnce("denied");
    const w = mountCard(ev("dump", { value_text: "x", file: "/srv/blog/app/Services/GitHubClient.php", line: 57 }));
    const caller = w.get('[data-testid="dump-caller"]');
    expect(caller.element.tagName).toBe("BUTTON");
    expect(caller.text()).toBe("GitHubClient.php:57");
    await caller.trigger("click");
    await flushPromises();
    expect(openPath).toHaveBeenCalledWith("/srv/blog/app/Services/GitHubClient.php");
    const toast = useToast().toasts.value[useToast().toasts.value.length - 1];
    expect(toast?.title).toBe("Couldn't open file");
    expect(toast?.detail).toBe("app/Services/GitHubClient.php:57: denied");
  });

  it("does not make a non-path caller clickable", () => {
    const w = mountCard(ev("dump", { value_text: "x", file: "Command line code", line: 1 }));
    expect(w.get('[data-testid="dump-caller"]').element.tagName).toBe("SPAN");
  });
});
