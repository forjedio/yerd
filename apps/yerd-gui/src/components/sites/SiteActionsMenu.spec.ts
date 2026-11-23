import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { defineComponent, h, nextTick, ref } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  showDumpsWindow: vi.fn(),
  listRoutes: vi.fn().mockResolvedValue([]),
  addRouteRule: vi.fn(),
  removeRouteRule: vi.fn(),
  pickDirectory: vi.fn(),
  openInBrowser: vi.fn(),
  mintWordPressLoginToken: vi.fn(),
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: vi.fn(), error: vi.fn(), info: vi.fn() }),
}));

import type { SiteEntry } from "@/ipc/types";
import SiteActionsMenu from "./SiteActionsMenu.vue";

const SITE: SiteEntry = {
  name: "blog",
  document_root: "/srv/blog",
  php: "8.3",
  secure: false,
  kind: "linked",
  is_laravel: false,
  uses_front_controller: true,
};

const layer = ref(false);
const shown = ref(true);
const mounted: VueWrapper[] = [];

function mountMenu() {
  const Host = defineComponent({
    setup() {
      return () =>
        shown.value
          ? h(SiteActionsMenu, {
              site: SITE,
              report: null,
              tld: "test",
              home: null,
              groups: { order: ["Work"], members: {} },
              currentGroup: "",
              parentDir: null,
              editorAvailable: false,
              ideOptions: [],
              selectedIde: "",
              layerOpen: layer.value,
              "onUpdate:layerOpen": (v: boolean) => {
                layer.value = v;
              },
            })
          : null;
    },
  });
  const w = mount(Host, { attachTo: document.body });
  mounted.push(w);
  return w;
}

function el(selector: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(selector);
  if (!found) throw new Error(`no ${selector}`);
  return found;
}

function byText(selector: string, text: string): HTMLElement {
  const found = Array.from(document.querySelectorAll<HTMLElement>(selector)).find((e) =>
    e.textContent?.includes(text),
  );
  if (!found) throw new Error(`no ${selector} with "${text}"`);
  return found;
}

async function settle(ms = 0): Promise<void> {
  await flushPromises();
  await new Promise((resolve) => setTimeout(resolve, ms));
  await nextTick();
}

async function openMenu(): Promise<void> {
  el('[aria-label="More actions for blog"]').dispatchEvent(
    new KeyboardEvent("keydown", { key: "Enter", bubbles: true }),
  );
  await settle();
}

beforeEach(() => {
  layer.value = false;
  shown.value = true;
});

afterEach(() => {
  mounted.splice(0).forEach((w) => w.unmount());
  document.body.innerHTML = "";
});

describe("SiteActionsMenu with real reka components", () => {
  it("keeps the New group popover open after the menu closes", async () => {
    mountMenu();
    await openMenu();
    const sub = byText('[role="menuitem"]', "Move to group");
    sub.dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true }));
    await settle();
    byText('[role="menuitem"]', "New group…").click();
    await settle(250);
    expect(document.querySelector('[data-testid="new-group-popover"]')).not.toBeNull();
    expect(document.querySelector('[role="menu"]')).toBeNull();
  });

  it("restores focus to the trigger when the menu closes normally", async () => {
    mountMenu();
    await openMenu();
    byText('[role="menuitem"]', "Copy URL").click();
    await settle(50);
    expect(document.activeElement).toBe(el('[aria-label="More actions for blog"]'));
  });

  it.each([
    ["Apply .yerd.toml", "applyProjectFile"],
    ["Export to .yerd.toml", "exportProjectFile"],
  ])("emits %s for the site", async (label, event) => {
    const w = mountMenu();
    await openMenu();
    byText('[role="menuitem"]', label).click();
    await settle();
    expect(w.findComponent(SiteActionsMenu).emitted(event)).toEqual([[SITE]]);
  });

  it("clears layerOpen when unmounted while the menu is open", async () => {
    mountMenu();
    await openMenu();
    expect(layer.value).toBe(true);
    shown.value = false;
    await settle();
    expect(layer.value).toBe(false);
  });
});
