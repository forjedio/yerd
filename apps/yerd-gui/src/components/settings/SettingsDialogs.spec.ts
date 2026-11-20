import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  getGuiLogs: vi.fn(),
  getDiagnostics: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  getGuiLogs: mocks.getGuiLogs,
  getDiagnostics: mocks.getDiagnostics,
}));

import { useToast } from "@/composables/useToast";

import ConnectAgentDialog from "./ConnectAgentDialog.vue";
import DiagnosticsDialog from "./DiagnosticsDialog.vue";
import GuiLogsDialog from "./GuiLogsDialog.vue";

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];
const clipboardWriteText = vi.fn();

function mountDialog(component: unknown, props: Record<string, unknown>) {
  const w = mount(component as Parameters<typeof mount>[0], {
    attachTo: document.body,
    props: { open: true, ...props },
    global: { stubs: { DialogPortal: Pass } },
  }) as VueWrapper;
  mounted.push(w);
  return w;
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

function button(w: VueWrapper, label: string) {
  const found = w.findAll("button").find((b) => b.text() === label);
  if (!found) throw new Error(`no button ${label}`);
  return found;
}

beforeEach(() => {
  vi.useFakeTimers();
  mocks.getGuiLogs.mockReset().mockResolvedValue({
    guiPath: "/logs/gui.log",
    guiLog: ["gui one", "gui two"],
    daemonPath: "/logs/daemon.log",
    daemonLog: ["daemon one"],
  });
  mocks.getDiagnostics.mockReset().mockResolvedValue("paths: ok");
  clipboardWriteText.mockReset().mockResolvedValue(undefined);
  Object.defineProperty(navigator, "clipboard", {
    configurable: true,
    value: { writeText: clipboardWriteText },
  });
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
  vi.useRealTimers();
});

describe("ConnectAgentDialog", () => {
  it("shows both snippets and copies each with its toast", async () => {
    const w = mountDialog(ConnectAgentDialog, { needsCliPath: false });
    expect(w.find('[data-testid="mcp-command"]').text()).toBe(
      "claude mcp add --scope user yerd -- yerd mcp",
    );
    expect(w.find('[data-testid="mcp-config"]').text()).toContain('"mcpServers"');

    await w.find('[aria-label="Copy command"]').trigger("click");
    await flushPromises();
    await w.find('[aria-label="Copy config"]').trigger("click");
    await flushPromises();

    expect(clipboardWriteText).toHaveBeenNthCalledWith(
      1,
      "claude mcp add --scope user yerd -- yerd mcp",
    );
    expect(JSON.parse(clipboardWriteText.mock.calls[1]?.[0] as string)).toEqual({
      mcpServers: { yerd: { command: "yerd", args: ["mcp"] } },
    });
    expect(toastTitles()).toEqual(["Command copied", "Config copied"]);
  });

  it("shows only the PATH note when the CLI isn't installed", () => {
    const w = mountDialog(ConnectAgentDialog, { needsCliPath: true });
    expect(w.find('[data-testid="mcp-needs-cli"]').exists()).toBe(true);
    expect(w.find('[data-testid="mcp-command"]').exists()).toBe(false);
  });

  it("closes on Done", async () => {
    const w = mountDialog(ConnectAgentDialog, { needsCliPath: false });
    await button(w, "Done").trigger("click");
    expect(w.emitted("update:open")).toEqual([[false]]);
  });
});

describe("GuiLogsDialog", () => {
  it("reads on open, polls every 2s and stops after close", async () => {
    const w = mountDialog(GuiLogsDialog, {});
    await flushPromises();
    expect(mocks.getGuiLogs).toHaveBeenCalledTimes(1);
    expect(w.find('[data-testid="gui-log"]').text()).toBe("gui one\ngui two");

    await vi.advanceTimersByTimeAsync(2000);
    expect(mocks.getGuiLogs).toHaveBeenCalledTimes(2);

    await w.setProps({ open: false });
    await vi.advanceTimersByTimeAsync(6000);
    expect(mocks.getGuiLogs).toHaveBeenCalledTimes(2);
  });

  it("switches to the daemon lines and path, and copies them", async () => {
    const w = mountDialog(GuiLogsDialog, {});
    await flushPromises();
    await w.findAll('[role="radio"]')[1]?.trigger("click");
    expect(w.find('[data-testid="gui-log"]').text()).toBe("daemon one");
    expect(w.find('[data-testid="gui-log-path"]').text()).toBe("/logs/daemon.log");

    await button(w, "Copy").trigger("click");
    await flushPromises();
    expect(clipboardWriteText).toHaveBeenCalledWith("daemon one");
  });

  it("toasts a failed first read but not a failed poll", async () => {
    mocks.getGuiLogs.mockRejectedValue(new Error("boom"));
    mountDialog(GuiLogsDialog, {});
    await flushPromises();
    await vi.advanceTimersByTimeAsync(4000);
    expect(mocks.getGuiLogs).toHaveBeenCalledTimes(3);
    expect(toastTitles()).toEqual(["Couldn't read logs"]);
  });
});

describe("DiagnosticsDialog", () => {
  it("gathers on open and copies the text", async () => {
    let resolve: (v: string) => void = () => {};
    mocks.getDiagnostics.mockReturnValue(new Promise<string>((r) => (resolve = r)));
    const w = mountDialog(DiagnosticsDialog, {});
    await flushPromises();
    expect(mocks.getDiagnostics).toHaveBeenCalledTimes(1);
    expect(button(w, "Copy").attributes("disabled")).toBeDefined();

    resolve("paths: ok");
    await flushPromises();
    expect(w.find('[data-testid="diagnostics"]').text()).toBe("paths: ok");
    await button(w, "Copy").trigger("click");
    await flushPromises();
    expect(clipboardWriteText).toHaveBeenCalledWith("paths: ok");
  });
});
