import { flushPromises, mount, type VueWrapper } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const client = vi.hoisted(() => ({ s3Credentials: vi.fn() }));

vi.mock("@/ipc/client", () => ({ IpcError: class IpcError extends Error {}, ...client }));

import ServiceCredentialsCard from "./ServiceCredentialsCard.vue";
import { useToast } from "@/composables/useToast";
import type { ServiceStatus } from "@/ipc/types";

const clipboardWriteText = vi.fn();
const mounted: VueWrapper[] = [];

function s3(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: "versitygw",
    type_id: "versitygw",
    display_name: "S3 (versitygw)",
    installed_versions: ["1.7.0"],
    selected_version: "1.7.0",
    state: "running",
    pid: 1,
    listen: "127.0.0.1:7070",
    port: 7070,
    enabled: true,
    supports_databases: false,
    ...overrides,
  };
}

const CREDS = { endpoint: "http://127.0.0.1:7070", access_key: "yerd_7Kq2XH9PmA" };
const SECRET = "Vq3uN8rT0xLm4Kp9Zs2Hd7Wy";

async function mountCard(props: Record<string, unknown> = {}) {
  const w = mount(ServiceCredentialsCard, {
    attachTo: document.body,
    props: { service: s3(), credentials: CREDS, ...props },
  });
  mounted.push(w);
  await flushPromises();
  return w;
}

beforeEach(() => {
  client.s3Credentials.mockReset().mockResolvedValue({ ...CREDS, secret_key: SECRET });
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
});

describe("ServiceCredentialsCard", () => {
  it("shows the endpoint and access key with the secret masked, without asking for it", async () => {
    const w = await mountCard();
    expect(w.get('[data-testid="cred-endpoint"]').text()).toBe("http://127.0.0.1:7070");
    expect(w.get('[data-testid="cred-access"]').text()).toBe("yerd_7Kq2XH9PmA");
    expect(w.get('[data-testid="cred-secret"]').text()).toBe("•".repeat(16));
    expect(w.text()).not.toContain(SECRET);
    expect(client.s3Credentials).not.toHaveBeenCalled();
  });

  it("reveals the secret on request and drops it on hide", async () => {
    const w = await mountCard();
    await w.get('[aria-label="Reveal secret key"]').trigger("click");
    await flushPromises();
    expect(client.s3Credentials).toHaveBeenCalledWith(true);
    expect(w.get('[data-testid="cred-secret"]').text()).toBe(SECRET);
    await w.get('[aria-label="Hide secret key"]').trigger("click");
    expect(w.get('[data-testid="cred-secret"]').text()).toBe("•".repeat(16));
    expect(w.text()).not.toContain(SECRET);
  });

  it.each([
    ["Copy endpoint", "http://127.0.0.1:7070"],
    ["Copy access key", "yerd_7Kq2XH9PmA"],
  ])("%s copies the value", async (label, expected) => {
    const w = await mountCard();
    await w.get(`[aria-label="${label}"]`).trigger("click");
    await flushPromises();
    expect(clipboardWriteText).toHaveBeenCalledWith(expected);
    expect(client.s3Credentials).not.toHaveBeenCalled();
    expect(useToast().toasts.value.map((t) => t.title)).toContain("Copied to clipboard");
  });

  it("copies the secret without revealing it", async () => {
    const w = await mountCard();
    await w.get('[aria-label="Copy secret key"]').trigger("click");
    await flushPromises();
    expect(client.s3Credentials).toHaveBeenCalledWith(true);
    expect(clipboardWriteText).toHaveBeenCalledWith(SECRET);
    expect(w.get('[data-testid="cred-secret"]').text()).toBe("•".repeat(16));
  });

  it("falls back to the configured port before credentials load", async () => {
    const w = await mountCard({ credentials: null, service: s3({ port: 7071 }) });
    expect(w.get('[data-testid="cred-endpoint"]').text()).toBe("http://127.0.0.1:7071");
    expect(w.get('[aria-label="Copy access key"]').attributes("disabled")).toBeDefined();
  });
});
