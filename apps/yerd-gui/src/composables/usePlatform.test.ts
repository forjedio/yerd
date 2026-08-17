import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Mock the IPC client's `hostPlatform` probe (the only dependency). Hoisted so
// the mock function is shared across the dynamic re-imports each test does.
const mocks = vi.hoisted(() => ({
  hostPlatform: vi.fn(),
}));

/** A `host_platform` response for `os`, with a vocab the daemon would send. */
function response(os: string) {
  const windows = os === "windows";
  return {
    os,
    vocab: {
      runtime: windows ? "php-cgi" : "PHP-FPM",
      pool: windows ? "FastCGI process" : "FPM pool",
      pools: windows ? "FastCGI processes" : "FPM pools",
      poolShort: windows ? "php-cgi" : "FPM",
      extSuffix: windows ? ".dll" : ".so",
      extExample: windows
        ? "C:\\php\\ext\\php_scrypt.dll"
        : "/opt/homebrew/lib/php/pecl/20250925/scrypt.so",
    },
  };
}

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
    mocks.hostPlatform.mockResolvedValue(response("windows"));
    const { loadPlatform, usePlatform } = await freshModule();
    await loadPlatform();

    const p = usePlatform();
    expect(p.platform.value).toBe("windows");
    expect(p.isWindows.value).toBe(true);
    expect(p.isMac.value).toBe(false);
    expect(p.isLinux.value).toBe(false);
    expect(p.supportsPathInstall.value).toBe(true);
    expect(p.vocab.value.pool).toBe("FastCGI process");
    expect(p.vocab.value.extSuffix).toBe(".dll");
  });

  it("flags macos with path-install support", async () => {
    mocks.hostPlatform.mockResolvedValue(response("macos"));
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

    mocks.hostPlatform.mockResolvedValueOnce(response("windows"));
    await loadPlatform();
    expect(usePlatform().isWindows.value).toBe(true);
    expect(mocks.hostPlatform).toHaveBeenCalledTimes(2);
  });

  // The seeded default is what renders before `host_platform` resolves. It must
  // match the Unix row, which is the behaviour the GUI had when `platform`
  // started empty. The per-OS table itself is pinned by
  // `crates/yerd-core/src/php_vocab.rs`, not here.
  it("seeds the unix vocabulary as the pre-load default", async () => {
    const { usePlatform } = await freshModule();
    const p = usePlatform();
    expect(p.platform.value).toBe("");
    expect(p.vocab.value).toEqual({
      runtime: "PHP-FPM",
      pool: "FPM pool",
      pools: "FPM pools",
      poolShort: "FPM",
      extSuffix: ".so",
      extExample: "/opt/homebrew/lib/php/pecl/20250925/scrypt.so",
    });
  });
});
