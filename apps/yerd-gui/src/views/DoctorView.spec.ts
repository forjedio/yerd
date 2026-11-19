import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { computed, ref } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const invokeMock = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

const hostPlatform = ref("macos");
vi.mock("@/composables/usePlatform", () => ({
  loadPlatform: () => Promise.resolve(),
  usePlatform: () => ({
    platform: hostPlatform,
    isMac: computed(() => hostPlatform.value === "macos"),
    isLinux: computed(() => hostPlatform.value === "linux"),
    supportsPathInstall: computed(() => hostPlatform.value === "macos" || hostPlatform.value === "linux"),
    home: ref("/Users/a"),
  }),
}));

import DoctorView from "./DoctorView.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import type { Diagnosis, StatusReport } from "@/ipc/types";

const FPM: Diagnosis = {
  code: "fpm_pool_failed",
  severity: "fail",
  title: "PHP-FPM pool failed",
  detail: "The PHP 8.4 FPM pool is not running.",
  remedy: "yerd use 8.4",
};
const PATH: Diagnosis = {
  code: "bin_dir_not_on_path",
  severity: "warn",
  title: "Yerd's bin directory isn't on PATH",
  detail: "Dev tool commands won't resolve in your shell",
  remedy: "yerd path install",
};
const UPDATE: Diagnosis = {
  code: "php_update_available",
  severity: "ok",
  title: "PHP update available",
  detail: "PHP 8.4 can be updated to 8.4.15",
  remedy: null,
};
const ALL_GOOD: Diagnosis = { code: "all_good", severity: "ok", title: "All checks passed", detail: "", remedy: null };

function report(over: Partial<StatusReport> = {}): StatusReport {
  return {
    tld: "test",
    http: { requested: 80, bound: 80, fell_back: false },
    https: { requested: 443, bound: 443, fell_back: false },
    port_redirect: false,
    ca: { path: "", fingerprint: "", trusted_system: true },
    resolver_installed: true,
    web_unbound: null,
    ...over,
  } as unknown as StatusReport;
}

interface StubOpts {
  diagnose?: () => Promise<unknown>;
  untrustResidual?: boolean;
}

let findings: Diagnosis[] = [];

function stubIpc(opts: StubOpts = {}) {
  invokeMock.mockImplementation((cmd: string) => {
    switch (cmd) {
      case "diagnose":
        return opts.diagnose?.() ?? Promise.resolve({ type: "diagnoses", items: findings });
      case "doctor_fix":
        return Promise.resolve({
          type: "doctor_fix",
          report: { performed: [{ code: "fpm_pool_failed", ok: true, message: "" }], manual: [PATH] },
        });
      case "status":
        return Promise.resolve({ type: "status", report: useDaemon().report.value });
      case "untrust_ca":
        return Promise.resolve(opts.untrustResidual ?? false);
      case "trust_ca":
      case "elevate":
      case "elevate_all":
      case "elevate_resolver_ports":
      case "unelevate":
        return Promise.resolve(undefined);
      default:
        return Promise.reject(new Error(`unexpected invoke ${cmd}`));
    }
  });
}

const Pass = { template: "<div><slot /></div>" };
const mounted: VueWrapper[] = [];

async function mountView() {
  const wrapper = mount(DoctorView, {
    attachTo: document.body,
    global: { stubs: { teleport: true, AlertDialogPortal: Pass } },
  });
  mounted.push(wrapper);
  await flushPromises();
  return wrapper;
}

type View = Awaited<ReturnType<typeof mountView>>;

function button(scope: View | ReturnType<View["get"]>, text: string) {
  const btn = scope.findAll("button").find((b) => b.text().trim() === text);
  if (!btn) throw new Error(`no button "${text}"`);
  return btn;
}

function inspector(w: View) {
  return w.get('[data-testid="doctor-inspector"]');
}

async function select(w: View, key: string) {
  await w.get(`[data-testid="doctor-list"] [data-key="${key}"]`).trigger("click");
  await flushPromises();
}

function dialogButton(text: string): HTMLButtonElement {
  const btn = Array.from(document.body.querySelectorAll('[role="alertdialog"] button')).find(
    (b) => b.textContent?.trim() === text,
  );
  if (!btn) throw new Error(`no dialog button "${text}"`);
  return btn as HTMLButtonElement;
}

function calls(cmd: string) {
  return invokeMock.mock.calls.filter((c) => c[0] === cmd);
}

function toastTitles(): string[] {
  return useToast().toasts.value.map((t) => t.title);
}

beforeEach(() => {
  invokeMock.mockReset();
  findings = [UPDATE, PATH, FPM, ALL_GOOD];
  hostPlatform.value = "macos";
  useDaemon().report.value = report();
  for (const t of [...useToast().toasts.value]) useToast().dismiss(t.id);
  stubIpc();
});

afterEach(() => {
  mounted.forEach((w) => w.unmount());
  mounted.length = 0;
  document.body.innerHTML = "";
});

describe("DoctorView findings", () => {
  it("groups findings with counts and selects the first problem", async () => {
    const w = await mountView();
    const labels = w.findAll('[data-testid="doctor-list"] section > p, [data-testid="doctor-list"] section > div > p');
    expect(labels.map((l) => l.text())).toEqual([
      "Problems · 1",
      "Warnings · 1",
      "Info · 1",
      "System permissions",
    ]);
    expect(w.text()).not.toContain("All checks passed");
    expect(inspector(w).attributes("data-state")).toBe("finding");
    expect(inspector(w).get('[data-testid="doctor-chip"]').text()).toBe("Problem");
    expect(inspector(w).text()).toContain("PHP-FPM pool failed");
  });

  it("switches the inspector on a row click and copies the remedy", async () => {
    const writeText = vi.fn().mockResolvedValue(undefined);
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const w = await mountView();
    await select(w, `bin_dir_not_on_path:${PATH.title}`);
    expect(inspector(w).text()).toContain(PATH.title);
    await inspector(w).get('[aria-label="Copy command"]').trigger("click");
    await flushPromises();
    expect(writeText).toHaveBeenCalledWith("yerd path install");
    expect(toastTitles()).toContain("Copied to clipboard");
  });

  it("shows the all-clear state and disables safe fixes with only info findings", async () => {
    findings = [UPDATE];
    const w = await mountView();
    expect(inspector(w).attributes("data-state")).toBe("clear");
    expect(inspector(w).text()).toContain("No problems found");
    expect(button(w, "Run safe fixes").attributes("disabled")).toBeDefined();
  });

  it("runs safe fixes, toasts the summary and re-runs the checks", async () => {
    const w = await mountView();
    await button(w, "Run safe fixes").trigger("click");
    await flushPromises();
    expect(calls("doctor_fix")).toHaveLength(1);
    expect(useToast().toasts.value.find((t) => t.title === "Ran safe fixes")?.detail).toBe(
      "1/1 applied · 1 need manual action",
    );
    expect(calls("diagnose")).toHaveLength(2);
  });
});

describe("DoctorView unavailable state", () => {
  it("shows the error state, then recovers on Re-check", async () => {
    let fail = true;
    stubIpc({
      diagnose: () => (fail ? Promise.reject(new Error("down")) : Promise.resolve({ type: "diagnoses", items: findings })),
    });
    const w = await mountView();
    expect(inspector(w).attributes("data-state")).toBe("error");
    expect(w.findAll('[data-testid="doctor-list"] section').map((s) => s.attributes("data-group"))).toEqual([
      "permissions",
    ]);
    fail = false;
    await button(inspector(w), "Re-check").trigger("click");
    await flushPromises();
    expect(toastTitles()).toContain("Health re-checked");
    expect(inspector(w).attributes("data-state")).toBe("finding");
  });

  it("shows the error state when a re-check fails after a successful load", async () => {
    let fail = false;
    stubIpc({
      diagnose: () => (fail ? Promise.reject(new Error("down")) : Promise.resolve({ type: "diagnoses", items: findings })),
    });
    const w = await mountView();
    fail = true;
    await w.get('[aria-label="Re-check health"]').trigger("click");
    await flushPromises();
    expect(inspector(w).attributes("data-state")).toBe("error");
    expect(button(w, "Run safe fixes").attributes("disabled")).toBeDefined();
  });
});

describe("DoctorView system permissions", () => {
  it.each([
    ["macos", "trust_ca", undefined],
    ["linux", "elevate", { target: "trust" }],
  ])("fixes CA trust on %s via %s and re-runs the checks", async (os, cmd, args) => {
    hostPlatform.value = os;
    useDaemon().report.value = report({ ca: { path: "", fingerprint: "", trusted_system: false } } as Partial<StatusReport>);
    const w = await mountView();
    await select(w, "perm:trust");
    expect(inspector(w).text()).toContain("Needs your password");
    await button(inspector(w), "Fix").trigger("click");
    await flushPromises();
    expect(invokeMock).toHaveBeenCalledWith(cmd, args);
    expect(calls("diagnose")).toHaveLength(2);
  });

  it("confirms Revert, does nothing on Cancel and shows the residual note on macOS", async () => {
    stubIpc({ untrustResidual: true });
    const w = await mountView();
    await select(w, "perm:trust");
    await button(inspector(w), "Revert").trigger("click");
    await flushPromises();
    expect(document.body.querySelector('[role="alertdialog"]')?.textContent).toContain("Untrust local CA");
    dialogButton("Cancel").click();
    await flushPromises();
    expect(calls("untrust_ca")).toHaveLength(0);

    await button(inspector(w), "Revert").trigger("click");
    await flushPromises();
    dialogButton("Revert").click();
    await flushPromises();
    expect(calls("untrust_ca")).toHaveLength(1);
    expect(inspector(w).get('[data-testid="doctor-note"]').text()).toContain("sudo yerd unelevate trust");
    expect(inspector(w).findAll("button").some((b) => b.text().trim() === "Revert")).toBe(false);
  });

  it("offers Fix all only for two or more fixes and runs only the needed steps on macOS", async () => {
    useDaemon().report.value = report({ resolver_installed: false });
    const w = await mountView();
    expect(w.find('[data-testid="doctor-fix-all"]').exists()).toBe(false);

    useDaemon().report.value = report({
      resolver_installed: false,
      ca: { path: "", fingerprint: "", trusted_system: false },
    } as Partial<StatusReport>);
    await flushPromises();
    await w.get('[data-testid="doctor-fix-all"]').trigger("click");
    await flushPromises();
    const rootCalls = invokeMock.mock.calls
      .map((c) => c[0])
      .filter((c) => c === "trust_ca" || c === "elevate_resolver_ports" || c === "elevate");
    expect(rootCalls).toEqual(["trust_ca", "elevate"]);
    expect(calls("elevate").map((c) => c[1])).toEqual([{ target: "resolver" }]);
  });

  it("shows a coming-soon pill instead of Fix where the platform can't elevate", async () => {
    hostPlatform.value = "windows";
    useDaemon().report.value = report({ resolver_installed: false });
    const w = await mountView();
    await select(w, "perm:resolver");
    expect(inspector(w).text()).toContain("soon");
    expect(inspector(w).findAll("button").some((b) => b.text().trim() === "Fix")).toBe(false);
  });

  it("shows the Settings note and no Fix for unbound web ports on macOS", async () => {
    useDaemon().report.value = report({ web_unbound: { http: 8080, https: 8443 } } as Partial<StatusReport>);
    const w = await mountView();
    await select(w, "perm:ports");
    expect(inspector(w).get('[data-testid="doctor-note"]').text()).toContain("Set working ports in Settings");
    expect(inspector(w).findAll("button").some((b) => b.text().trim() === "Fix")).toBe(false);
  });
});
