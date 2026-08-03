import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock the IPC client's `hostPlatform` probe (the only dependency). Hoisted so
// the mock function is shared across the dynamic re-imports each test does.
const mocks = vi.hoisted(() => ({
  hostPlatform: vi.fn(),
}));

vi.mock("@/ipc/client", () => ({
  hostPlatform: mocks.hostPlatform,
}));

// The composable caches the platform in a module-level singleton, so each test
// resets modules and re-imports to start from a clean `platform`/`loadPromise`.
async function freshModule() {
  vi.resetModules();
  return import("./usePlatform");
}

describe("usePlatform", () => {
  beforeEach(() => {
    mocks.hostPlatform.mockReset();
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("flags windows with path-install support and leaves mac/linux false", async () => {
    mocks.hostPlatform.mockResolvedValue("windows");
    const { loadPlatform, usePlatform } = await freshModule();
    await loadPlatform();

    const p = usePlatform();
    expect(p.platform.value).toBe("windows");
    expect(p.isWindows.value).toBe(true);
    expect(p.isMac.value).toBe(false);
    expect(p.isLinux.value).toBe(false);
    expect(p.supportsPathInstall.value).toBe(true);
  });

  it("flags macos with path-install support", async () => {
    mocks.hostPlatform.mockResolvedValue("macos");
    const { loadPlatform, usePlatform } = await freshModule();
    await loadPlatform();

    const p = usePlatform();
    expect(p.isMac.value).toBe(true);
    expect(p.isWindows.value).toBe(false);
    expect(p.supportsPathInstall.value).toBe(true);
  });

  it("retries after a failed load", async () => {
    mocks.hostPlatform.mockRejectedValueOnce(new Error("daemon down"));
    const { loadPlatform, usePlatform } = await freshModule();
    await loadPlatform();
    expect(usePlatform().platform.value).toBe("");

    mocks.hostPlatform.mockResolvedValueOnce("windows");
    await loadPlatform();
    expect(usePlatform().isWindows.value).toBe(true);
    expect(mocks.hostPlatform).toHaveBeenCalledTimes(2);
  });
});
