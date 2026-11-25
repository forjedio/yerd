import { mount } from "@vue/test-utils";
import { describe, expect, it } from "vitest";

import ShareProviderCards from "./ShareProviderCards.vue";
import type { TunnelProviderStatus } from "@/ipc/types";

const providers: TunnelProviderStatus[] = [
  { provider: "cloudflare", readiness: "ready", configured: true },
  { provider: "ngrok", readiness: "ready", configured: true },
  { provider: "zrok", readiness: "needs_setup", configured: false },
];

describe("ShareProviderCards", () => {
  it("shows each provider's status and emits manage", async () => {
    const w = mount(ShareProviderCards, { props: { providers } });
    const status = (p: string) => w.get(`[data-testid="provider-card-${p}"] [data-testid="provider-card-status"]`).text();
    expect(status("cloudflare")).toBe("Connected");
    expect(status("ngrok")).toBe("Token saved");
    expect(status("zrok")).toBe("Not set up");
    await w.get('[data-testid="providers-manage"]').trigger("click");
    expect(w.emitted("manage")).toHaveLength(1);
  });

  it("leaves out providers that have not loaded", () => {
    const w = mount(ShareProviderCards, { props: { providers: [] } });
    expect(w.find('[data-testid="provider-card-cloudflare"]').exists()).toBe(false);
  });

  it("shows a load failure with a Retry instead of the cards", async () => {
    const w = mount(ShareProviderCards, { props: { providers: [], error: "daemon unreachable" } });
    expect(w.get('[data-testid="providers-error"]').text()).toContain("daemon unreachable");
    expect(w.find('[data-testid="provider-card-cloudflare"]').exists()).toBe(false);
    await w.get('[data-testid="providers-retry"]').trigger("click");
    expect(w.emitted("retry")).toHaveLength(1);
  });
});
