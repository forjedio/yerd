import { flushPromises, mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import ZrokPane from "./ZrokPane.vue";
import type { TunnelProviderStatus } from "@/ipc/types";

function status(extra: Partial<TunnelProviderStatus> = {}): TunnelProviderStatus {
  return { provider: "zrok", readiness: "needs_setup", version: "2.0.11", source: "managed", configured: false, ...extra };
}

async function mountPane(s: TunnelProviderStatus = status()) {
  const wrapper = mount(ZrokPane, { props: { status: s, busy: null, connected: true } });
  await flushPromises();
  return wrapper;
}

describe("ZrokPane", () => {
  it("disables Enable while the token is empty", async () => {
    const w = await mountPane();
    expect(w.get('[data-action="enable"]').attributes("disabled")).toBeDefined();
    expect(w.get('[data-testid="binary-source"]').text()).toBe("Yerd's copy");
  });

  it("disables Enable while zrok is not installed", async () => {
    const w = await mountPane({ provider: "zrok", readiness: "not_installed", configured: false });
    await w.get('input[aria-label="zrok account token"]').setValue("tok");
    expect(w.get('[data-action="enable"]').attributes("disabled")).toBeDefined();
    expect(w.find('[data-action="tooling"]').exists()).toBe(true);
  });

  it("emits the trimmed token and clears it once enabled", async () => {
    const w = await mountPane();
    await w.get('input[aria-label="zrok account token"]').setValue("  tok123 ");
    await w.get('[data-action="enable"]').trigger("click");
    expect(w.emitted("enable")).toEqual([["tok123"]]);
    await w.setProps({ status: status({ configured: true, readiness: "ready" }) });
    expect(w.find('[data-testid="zrok-enabled"]').exists()).toBe(true);
    await w.setProps({ status: status() });
    expect((w.get('input[aria-label="zrok account token"]').element as HTMLInputElement).value).toBe("");
  });

  it("disables an enabled machine", async () => {
    const w = await mountPane(status({ configured: true, readiness: "ready" }));
    await w.get('[data-action="disable"]').trigger("click");
    expect(w.emitted("disable")).toHaveLength(1);
  });
});
