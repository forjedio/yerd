import { flushPromises } from "@vue/test-utils";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { effectScope, ref, type EffectScope } from "vue";

const listBuckets = vi.fn();
const bucketStats = vi.fn();
const s3Credentials = vi.fn();
vi.mock("@/ipc/client", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/ipc/client")>();
  return {
    ...actual,
    listBuckets: (...a: unknown[]) => listBuckets(...a),
    bucketStats: (...a: unknown[]) => bucketStats(...a),
    s3Credentials: (...a: unknown[]) => s3Credentials(...a),
  };
});

import { IpcError } from "@/ipc/client";
import type { ServiceStatus } from "@/ipc/types";
import { type S3Storage, useS3Storage } from "./useS3Storage";

function s3(overrides: Partial<ServiceStatus> = {}): ServiceStatus {
  return {
    service: "versitygw",
    type_id: "versitygw",
    display_name: "S3 (versitygw)",
    installed_versions: ["1.7.0"],
    selected_version: "1.7.0",
    state: "running",
    pid: 7,
    listen: "127.0.0.1:7070",
    port: 7070,
    enabled: true,
    supports_databases: false,
    ...overrides,
  };
}

const sizes: Record<string, { objects: number; bytes: number }> = {
  avatars: { objects: 1284, bytes: 312_400_000 },
  invoices: { objects: 0, bytes: 0 },
};

let scope: EffectScope;

function setup(initial: ServiceStatus | null): { service: ReturnType<typeof ref<ServiceStatus | null>>; s3: S3Storage } {
  const service = ref<ServiceStatus | null>(initial);
  scope = effectScope();
  const storage = scope.run(() => useS3Storage(service)) as S3Storage;
  return { service, s3: storage };
}

beforeEach(() => {
  listBuckets.mockReset().mockResolvedValue(["avatars", "invoices"]);
  bucketStats.mockReset().mockImplementation((name: string) => Promise.resolve(sizes[name]));
  s3Credentials
    .mockReset()
    .mockResolvedValue({ endpoint: "http://127.0.0.1:7070", access_key: "yerd_7Kq2XH9PmA" });
});

afterEach(() => scope.stop());

describe("useS3Storage", () => {
  it("lists buckets and fetches every bucket's stats when running", async () => {
    const { s3: storage } = setup(s3());
    await flushPromises();
    expect(listBuckets).toHaveBeenCalledTimes(1);
    expect(bucketStats.mock.calls.map((c) => c[0])).toEqual(["avatars", "invoices"]);
    expect(storage.state.value.buckets).toEqual({ names: ["avatars", "invoices"], loading: false, error: null });
    expect(storage.state.value.stats).toEqual(sizes);
  });

  it("lists only on the running transition, not on every service update", async () => {
    const { service } = setup(s3({ state: "stopped" }));
    await flushPromises();
    expect(listBuckets).not.toHaveBeenCalled();
    service.value = s3();
    await flushPromises();
    service.value = s3({ pid: 8 });
    await flushPromises();
    expect(listBuckets).toHaveBeenCalledTimes(1);
  });

  it("clears buckets and stats when the service stops", async () => {
    const { service, s3: storage } = setup(s3());
    await flushPromises();
    service.value = s3({ state: "stopped" });
    await flushPromises();
    expect(storage.state.value.buckets.names).toBeNull();
    expect(storage.state.value.stats).toEqual({});
  });

  it("drops a stale reply overtaken by a stop", async () => {
    let resolve: (v: string[]) => void = () => {};
    listBuckets.mockReturnValue(new Promise<string[]>((r) => (resolve = r)));
    const { service, s3: storage } = setup(s3());
    await flushPromises();
    service.value = s3({ state: "stopped" });
    await flushPromises();
    resolve(["avatars"]);
    await flushPromises();
    expect(storage.state.value.buckets.names).toBeNull();
    expect(bucketStats).not.toHaveBeenCalled();
  });

  it("drops a stale reply overtaken by a reload", async () => {
    let first: (v: string[]) => void = () => {};
    listBuckets.mockReturnValueOnce(new Promise<string[]>((r) => (first = r)));
    const { s3: storage } = setup(s3());
    await flushPromises();
    listBuckets.mockResolvedValueOnce(["backups"]);
    storage.reload();
    await flushPromises();
    first(["old"]);
    await flushPromises();
    expect(storage.state.value.buckets.names).toEqual(["backups"]);
  });

  it("records a failed listing and a failed stat", async () => {
    listBuckets.mockRejectedValueOnce(new IpcError("start S3 (versitygw) first", "internal"));
    const { s3: storage } = setup(s3());
    await flushPromises();
    expect(storage.state.value.buckets).toEqual({ names: null, loading: false, error: "start S3 (versitygw) first" });
    bucketStats.mockImplementation((name: string) =>
      name === "avatars" ? Promise.reject(new IpcError("boom")) : Promise.resolve(sizes[name]),
    );
    storage.reload();
    await flushPromises();
    expect(storage.state.value.stats).toEqual({ avatars: "error", invoices: sizes.invoices });
  });

  it("removes a dropped bucket, then lists again", async () => {
    const { s3: storage } = setup(s3());
    await flushPromises();
    expect(listBuckets).toHaveBeenCalledTimes(1);
    storage.bucketDropped("avatars");
    expect(storage.state.value.buckets.names).toEqual(["invoices"]);
    expect(storage.state.value.stats).toEqual({ invoices: sizes.invoices });
    expect(listBuckets).toHaveBeenCalledTimes(2);
  });

  it("loads non-secret credentials and reloads them when the port changes", async () => {
    const { service, s3: storage } = setup(s3({ state: "stopped" }));
    await flushPromises();
    expect(s3Credentials).toHaveBeenCalledWith(false);
    expect(storage.state.value.credentials).toEqual({
      endpoint: "http://127.0.0.1:7070",
      access_key: "yerd_7Kq2XH9PmA",
    });
    s3Credentials.mockResolvedValue({ endpoint: "http://127.0.0.1:7071", access_key: "yerd_7Kq2XH9PmA" });
    service.value = s3({ state: "stopped", port: 7071 });
    await flushPromises();
    expect(s3Credentials).toHaveBeenCalledTimes(2);
    expect(storage.state.value.credentials?.endpoint).toBe("http://127.0.0.1:7071");
  });

  it("holds nothing without an installed instance", async () => {
    const { s3: storage } = setup(s3({ installed_versions: [], selected_version: null, state: "stopped" }));
    await flushPromises();
    expect(s3Credentials).not.toHaveBeenCalled();
    expect(storage.state.value.credentials).toBeNull();
    expect(listBuckets).not.toHaveBeenCalled();
  });
});
