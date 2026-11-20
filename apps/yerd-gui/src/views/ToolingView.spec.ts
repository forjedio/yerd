import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

const revealMock = vi.fn();
vi.mock("@tauri-apps/plugin-opener", () => ({
  revealItemInDir: (...args: unknown[]) => revealMock(...args),
  openUrl: vi.fn(),
  openPath: vi.fn(),
}));

import ToolingView from "./ToolingView.vue";
import { useOperations } from "@/composables/useOperations";
import { resetResourceCache } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import type { ToolStatus } from "@/ipc/types";

const Pass = { template: "<div><slot /></div>" };

const NAMES: Record<string, string> = {
  node: "Node.js",
  composer: "Composer",
  bun: "Bun",
  pnpm: "pnpm",
  laravel: "Laravel Installer",
  "wp-cli": "WP-CLI",
};

const BINS: Record<string, string[]> = {
  node: ["node", "npm", "npx"],
  composer: ["composer"],
  bun: ["bun", "bunx"],
  pnpm: ["pnpm", "pnpx"],
  laravel: ["laravel"],
  "wp-cli": ["wp"],
};

function tool(id: string, extra: Partial<ToolStatus> = {}): ToolStatus {
  return {
    id,
    display_name: NAMES[id] ?? id,
    installed: false,
    version: null,
    binaries: BINS[id] ?? [id],
    ...extra,
  };
}

function installed(id: string, version: string, extra: Partial<ToolStatus> = {}): ToolStatus {
  return tool(id, { installed: true, version, ...extra });
}

interface Job {
  lines: string[];
  final: { state: "succeeded" | "failed" | "cancelled"; error?: string } | null;
}

let job: Job = { lines: [], final: null };

function stubIpc(opts: { tools: ToolStatus[]; after?: ToolStatus[] }) {
  let current = opts.tools;
  job = { lines: [], final: null };
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "list_tools":
        return Promise.resolve({ type: "tools", tools: current });
      case "install_tool_streamed":
        current = opts.after ?? current;
        return Promise.resolve({ type: "job_started", job_id: "j1" });
      case "uninstall_tool":
        current = opts.after ?? current;
        return Promise.resolve({ type: "ok" });
      case "job_status": {
        const log = job.lines;
        job.lines = [];
        return Promise.resolve({
          type: "job_progress",
          state: job.final?.state ?? "running",
          phase: "",
          log,
          next_cursor: 1,
          error: job.final?.error ?? null,
        });
      }
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

const mounted: VueWrapper[] = [];

async function mountView() {
  const wrapper = mount(ToolingView, {
    attachTo: document.body,
    global: {
      stubs: {
        teleport: true,
        RouterLink: true,
        DialogPortal: Pass,
        AlertDialogPortal: Pass,
      },
    },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

function buttonIn(wrapper: VueWrapper, testId: string, label: string) {
  const b = wrapper
    .get(`[data-testid="${testId}"]`)
    .findAll("button")
    .find((x) => x.text().trim() === label);
  if (!b) throw new Error(`${label} button not rendered in ${testId}`);
  return b;
}

function confirmButton(wrapper: VueWrapper, label: string) {
  const b = wrapper
    .get('[role="alertdialog"]')
    .findAll("button")
    .find((x) => x.text().trim() === label);
  if (!b) throw new Error(`${label} button not rendered in the confirm`);
  return b;
}

function dialogClose(wrapper: VueWrapper) {
  const b = wrapper
    .get('[role="dialog"]')
    .findAll("button")
    .find((x) => x.text().trim() === "Close");
  if (!b) throw new Error("Close button not rendered in the log dialog");
  return b;
}

function callsOf(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

function toasts() {
  return useToast().toasts.value.map((t) => ({ title: t.title, detail: t.detail }));
}

async function finishJob(
  final: NonNullable<Job["final"]>,
  title: string,
): Promise<void> {
  job.final = final;
  await vi.waitFor(() => expect(toasts().map((t) => t.title)).toContain(title), {
    timeout: 2000,
  });
  await flushPromises();
}

function logText(wrapper: VueWrapper) {
  return wrapper.get('[data-testid="tool-install-log"]').text();
}

describe("ToolingView", () => {
  beforeEach(() => {
    invokeMock.mockReset();
    revealMock.mockReset();
    resetResourceCache();
    for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
    const ops = useOperations();
    ops.active.value.forEach((o) => ops.end(o.id));
  });

  afterEach(() => {
    mounted.forEach((w) => w.unmount());
    mounted.length = 0;
    document.body.innerHTML = "";
    const ops = useOperations();
    ops.active.value.forEach((o) => ops.end(o.id));
  });

  it("leaves Node to the Node page, installed or not", async () => {
    for (const nodeInstalled of [true, false]) {
      resetResourceCache();
      stubIpc({
        tools: [
          nodeInstalled ? installed("node", "22.9.0") : tool("node"),
          installed("composer", "2.8.4"),
          tool("bun"),
        ],
      });
      const wrapper = await mountView();
      expect(wrapper.find('[data-testid="tool-row-node"]').exists()).toBe(false);
      expect(wrapper.find('[data-testid="tool-card-node"]').exists()).toBe(false);
      expect(wrapper.get('[data-testid="tooling-installed-count"]').text()).toBe("1");
      expect(wrapper.get('[data-testid="tooling-available-count"]').text()).toBe("1");
    }
  });

  it("renders installed rows and available cards with chips and descriptions", async () => {
    stubIpc({
      tools: [installed("composer", "2.8.4"), installed("pnpm", "10.4.1"), tool("bun")],
    });
    const wrapper = await mountView();
    const composer = wrapper.get('[data-testid="tool-row-composer"]').text();
    expect(composer).toContain("Composer");
    expect(composer).toContain("2.8.4");
    expect(composer).toContain("PHP dependency manager");
    const pnpm = wrapper.get('[data-testid="tool-row-pnpm"]');
    expect(pnpm.findAll("code").map((c) => c.text())).toEqual(["pnpm", "pnpx"]);
    const bun = wrapper.get('[data-testid="tool-card-bun"]');
    expect(bun.text()).toContain("Fast JavaScript runtime and package manager");
    expect(bun.findAll("code").map((c) => c.text())).toEqual(["bun", "bunx"]);
    expect(bun.attributes("data-state")).toBe("idle");
    expect(wrapper.get('[data-testid="tooling-installed-count"]').text()).toBe("2");
    expect(wrapper.get('[data-testid="tooling-available-count"]').text()).toBe("1");
    expect(wrapper.get('[data-testid="tooling-installed"]').text()).toContain(
      "Node.js is managed from the Node page",
    );
  });

  it("enables Update only when an update is known, with a chip and title", async () => {
    stubIpc({
      tools: [
        installed("composer", "2.8.4", { update_available: "2.8.5" }),
        installed("pnpm", "10.4.1"),
      ],
    });
    const wrapper = await mountView();
    expect(wrapper.get('[data-testid="tool-update-composer"]').text()).toBe("↑ 2.8.5");
    const composerUpdate = buttonIn(wrapper, "tool-row-composer", "Update");
    expect(composerUpdate.attributes("title")).toBe("Update to 2.8.5");
    expect(composerUpdate.attributes("disabled")).toBeUndefined();
    expect(wrapper.find('[data-testid="tool-update-pnpm"]').exists()).toBe(false);
    const pnpmUpdate = buttonIn(wrapper, "tool-row-pnpm", "Update");
    expect(pnpmUpdate.attributes("title")).toBe("Up to date");
    expect(pnpmUpdate.attributes("disabled")).toBeDefined();
  });

  it("opens the log on Update and toasts the refreshed version", async () => {
    stubIpc({
      tools: [installed("composer", "1.0.0", { update_available: "2.0.0" })],
      after: [installed("composer", "2.0.0")],
    });
    job.lines = ["Installing Composer…"];
    const wrapper = await mountView();
    await buttonIn(wrapper, "tool-row-composer", "Update").trigger("click");
    await flushPromises();
    expect(callsOf("install_tool_streamed")[0]?.[1]).toEqual({ tool: "composer" });
    expect(wrapper.get('[data-testid="tool-install-title"]').text()).toBe("Updating Composer");
    expect(logText(wrapper)).toContain("Installing Composer…");
    await finishJob({ state: "succeeded" }, "Updated Composer 2.0.0");
    expect(toasts()).toContainEqual({
      title: "Updated Composer 2.0.0",
      detail: "composer is on your PATH",
    });
  });

  it("installs from a card without opening the log, then moves it to Installed", async () => {
    stubIpc({ tools: [tool("bun")], after: [installed("bun", "1.3.2")] });
    job.lines = ["Installing Bun…", "Fetching bun-darwin-aarch64.zip"];
    const wrapper = await mountView();
    await buttonIn(wrapper, "tool-card-bun", "Install").trigger("click");
    await flushPromises();
    expect(wrapper.find('[role="dialog"]').exists()).toBe(false);
    const card = wrapper.get('[data-testid="tool-card-bun"]');
    expect(card.attributes("data-state")).toBe("installing");
    expect(card.get('[data-testid="tool-card-detail"]').text()).toBe(
      "Fetching bun-darwin-aarch64.zip",
    );
    await buttonIn(wrapper, "tool-card-bun", "Show log").trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="tool-install-title"]').text()).toBe("Installing Bun");
    expect(logText(wrapper)).toContain("Installing Bun…");
    expect(logText(wrapper)).toContain("Fetching bun-darwin-aarch64.zip");
    await finishJob({ state: "succeeded" }, "Installed Bun 1.3.2");
    expect(toasts()).toContainEqual({
      title: "Installed Bun 1.3.2",
      detail: "bun and bunx are on your PATH",
    });
    expect(wrapper.find('[data-testid="tool-card-bun"]').exists()).toBe(false);
    expect(wrapper.find('[data-testid="tool-row-bun"]').exists()).toBe(true);
  });

  it("keeps polling after the log is closed mid-install", async () => {
    stubIpc({ tools: [tool("bun")], after: [installed("bun", "1.3.2")] });
    job.lines = ["Installing Bun…"];
    const wrapper = await mountView();
    await buttonIn(wrapper, "tool-card-bun", "Install").trigger("click");
    await flushPromises();
    await buttonIn(wrapper, "tool-card-bun", "Show log").trigger("click");
    await flushPromises();
    await dialogClose(wrapper).trigger("click");
    await flushPromises();
    expect(wrapper.find('[data-testid="tool-install-log"]').exists()).toBe(false);
    await finishJob({ state: "succeeded" }, "Installed Bun 1.3.2");
    expect(wrapper.find('[data-testid="tool-row-bun"]').exists()).toBe(true);
  });

  it("shows the failed state with the job error, its log and Try again", async () => {
    stubIpc({ tools: [tool("bun")] });
    job.lines = ["Installing Bun…", "Error: network down"];
    const wrapper = await mountView();
    await buttonIn(wrapper, "tool-card-bun", "Install").trigger("click");
    await flushPromises();
    await finishJob({ state: "failed", error: "network down" }, "Install of Bun failed");
    const card = wrapper.get('[data-testid="tool-card-bun"]');
    expect(card.attributes("data-state")).toBe("failed");
    expect(card.text()).toContain("Install failed");
    expect(card.get('[data-testid="tool-card-error"]').text()).toBe("network down");
    await buttonIn(wrapper, "tool-card-bun", "Show log").trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="tool-install-title"]').text()).toBe("Installing Bun");
    expect(wrapper.get('[role="dialog"]').find(".text-destructive").exists()).toBe(true);
    expect(logText(wrapper)).toContain("Error: network down");
    await dialogClose(wrapper).trigger("click");
    await flushPromises();
    for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
    stubIpc({ tools: [tool("bun")], after: [installed("bun", "1.3.2")] });
    await buttonIn(wrapper, "tool-card-bun", "Try again").trigger("click");
    await flushPromises();
    expect(callsOf("install_tool_streamed")).toHaveLength(2);
    expect(wrapper.get('[data-testid="tool-card-bun"]').attributes("data-state")).toBe(
      "installing",
    );
    await finishJob({ state: "succeeded" }, "Installed Bun 1.3.2");
  });

  it("disables every mutating action while one install runs", async () => {
    stubIpc({
      tools: [installed("composer", "2.8.4"), tool("bun"), tool("pnpm")],
      after: [installed("composer", "2.8.4"), installed("bun", "1.3.2"), tool("pnpm")],
    });
    const wrapper = await mountView();
    await buttonIn(wrapper, "tool-card-bun", "Install").trigger("click");
    await flushPromises();
    expect(buttonIn(wrapper, "tool-card-pnpm", "Install").attributes("disabled")).toBeDefined();
    expect(buttonIn(wrapper, "tool-row-composer", "Update").attributes("disabled")).toBeDefined();
    expect(
      wrapper.get('[aria-label="Uninstall Composer"]').attributes("disabled"),
    ).toBeDefined();
    await finishJob({ state: "succeeded" }, "Installed Bun 1.3.2");
    expect(buttonIn(wrapper, "tool-card-pnpm", "Install").attributes("disabled")).toBeUndefined();
  });

  it("shows a running install's last line after a remount", async () => {
    stubIpc({ tools: [tool("bun")] });
    useOperations().begin({
      id: "tool-install:bun",
      kind: "tool-install",
      label: "Installing Bun",
      detail: "Fetching bun-darwin-aarch64.zip",
    });
    const wrapper = await mountView();
    const card = wrapper.get('[data-testid="tool-card-bun"]');
    expect(card.attributes("data-state")).toBe("installing");
    expect(card.get('[data-testid="tool-card-detail"]').text()).toBe(
      "Fetching bun-darwin-aarch64.zip",
    );
    await buttonIn(wrapper, "tool-card-bun", "Show log").trigger("click");
    await flushPromises();
    expect(logText(wrapper)).toBe("Fetching bun-darwin-aarch64.zip");
  });

  it("gates the Composer-built tools on Yerd's own Composer", async () => {
    for (const composer of [null, tool("composer", { external: true })]) {
      resetResourceCache();
      stubIpc({ tools: [...(composer ? [composer] : []), tool("laravel")] });
      const wrapper = await mountView();
      expect(
        wrapper.find('[data-testid="tool-card-laravel"] [data-testid="tool-card-needs-composer"]')
          .exists(),
      ).toBe(true);
      expect(buttonIn(wrapper, "tool-card-laravel", "Install").attributes("disabled")).toBeDefined();
    }
  });

  it("installs Composer from the needs-Composer row", async () => {
    stubIpc({
      tools: [tool("composer"), tool("laravel")],
      after: [installed("composer", "2.8.4"), tool("laravel")],
    });
    const wrapper = await mountView();
    await buttonIn(wrapper, "tool-card-laravel", "install it first").trigger("click");
    await flushPromises();
    expect(callsOf("install_tool_streamed")[0]?.[1]).toEqual({ tool: "composer" });
    await finishJob({ state: "succeeded" }, "Installed Composer 2.8.4");
    expect(wrapper.find('[data-testid="tool-card-needs-composer"]').exists()).toBe(false);
  });

  it("disables Update on an installed Laravel installer without Composer", async () => {
    stubIpc({ tools: [installed("laravel", "5.10.0")] });
    const wrapper = await mountView();
    const update = buttonIn(wrapper, "tool-row-laravel", "Update");
    expect(update.attributes("disabled")).toBeDefined();
    expect(update.attributes("title")).toContain("Composer");
  });

  it("shows an external tool's location, reveals it and installs Yerd's copy", async () => {
    stubIpc({
      tools: [tool("bun", { external: true, external_path: "/opt/homebrew/bin/bun" })],
      after: [installed("bun", "1.3.2")],
    });
    const wrapper = await mountView();
    const card = wrapper.get('[data-testid="tool-card-bun"]');
    expect(card.text()).toContain("External");
    expect(card.get('[data-testid="tool-card-location"]').text()).toBe("/opt/homebrew/bin/bun");
    expect(card.get('[data-testid="tool-card-note"]').text()).toContain(
      "This is the bun your terminal runs now.",
    );
    await card.get('[aria-label="Reveal /opt/homebrew/bin/bun"]').trigger("click");
    await flushPromises();
    expect(revealMock).toHaveBeenCalledWith("/opt/homebrew/bin/bun");
    await buttonIn(wrapper, "tool-card-bun", "Install Yerd's copy").trigger("click");
    await flushPromises();
    expect(callsOf("install_tool_streamed")[0]?.[1]).toEqual({ tool: "bun" });
    await finishJob({ state: "succeeded" }, "Installed Bun 1.3.2");
  });

  it("keeps the note but omits the location for an external tool without a path", async () => {
    stubIpc({ tools: [tool("pnpm", { external: true })] });
    const wrapper = await mountView();
    const card = wrapper.get('[data-testid="tool-card-pnpm"]');
    expect(card.find('[data-testid="tool-card-location"]').exists()).toBe(false);
    expect(card.find('[data-testid="tool-card-note"]').exists()).toBe(true);
  });

  it("calls nothing when the uninstall confirm is cancelled", async () => {
    stubIpc({ tools: [installed("bun", "1.3.2")] });
    const wrapper = await mountView();
    await wrapper.get('[aria-label="Uninstall Bun"]').trigger("click");
    await flushPromises();
    expect(wrapper.get('[data-testid="tool-uninstall-body"]').text()).toContain(
      "bun and bunx leave your PATH",
    );
    await confirmButton(wrapper, "Cancel").trigger("click");
    await flushPromises();
    expect(wrapper.find('[role="alertdialog"]').exists()).toBe(false);
    expect(callsOf("uninstall_tool")).toHaveLength(0);
  });

  it("uninstalls through the confirm and toasts", async () => {
    stubIpc({ tools: [installed("bun", "1.3.2")], after: [tool("bun")] });
    const wrapper = await mountView();
    await wrapper.get('[aria-label="Uninstall Bun"]').trigger("click");
    await flushPromises();
    await confirmButton(wrapper, "Uninstall").trigger("click");
    await flushPromises();
    expect(callsOf("uninstall_tool")[0]?.[1]).toEqual({ tool: "bun" });
    expect(toasts()).toContainEqual({
      title: "Removed Bun",
      detail: "Its commands are off your PATH",
    });
    expect(wrapper.find('[data-testid="tool-card-bun"]').exists()).toBe(true);
  });

  it("shows the empty states", async () => {
    stubIpc({ tools: [tool("node"), tool("bun")] });
    const empty = await mountView();
    expect(empty.get('[data-testid="tooling-installed"]').text()).toContain(
      "No tools installed yet",
    );
    resetResourceCache();
    stubIpc({ tools: [installed("bun", "1.3.2")] });
    const full = await mountView();
    expect(full.get('[data-testid="tooling-available"]').text()).toContain(
      "Everything's installed",
    );
  });
});
