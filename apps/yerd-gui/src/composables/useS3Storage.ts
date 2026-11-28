import { computed, ref, watch, type ComputedRef, type Ref } from "vue";

import { bucketStats, IpcError, listBuckets, s3Credentials } from "@/ipc/client";
import type { BucketStats, ServiceStatus } from "@/ipc/types";
import { isInstalled } from "@/lib/services";

/** The bucket list: names once listed, null while unknown or stopped. */
export interface S3BucketList {
  names: string[] | null;
  loading: boolean;
  error: string | null;
}

/** One bucket's stats, or where fetching them stands. */
export type S3BucketStat = BucketStats | "loading" | "error";

/** The endpoint and access key; the secret is never held at view level. */
export interface S3PublicCredentials {
  endpoint: string;
  access_key: string;
}

/** Everything the Services view shows about S3 storage. */
export interface S3StorageState {
  buckets: S3BucketList;
  stats: Record<string, S3BucketStat>;
  credentials: S3PublicCredentials | null;
}

/** The composable's public surface. */
export interface S3Storage {
  state: ComputedRef<S3StorageState>;
  reload: () => void;
  bucketDropped: (name: string) => void;
}

/**
 * The S3 gateway's buckets, per-bucket stats and non-secret credentials for
 * the Services view, mirroring the view's database lists: buckets are listed
 * when the service turns running (once per run), on {@link S3Storage.reload}
 * (⌘R, after a create) and never on the 5s poll; stopping clears them. Every
 * bucket's stats are fetched together once the names arrive. A sequence
 * number drops replies that a newer listing or a stop has overtaken.
 * Credentials load while an installed instance exists and reload when its
 * port or run state changes.
 */
export function useS3Storage(service: Ref<ServiceStatus | null>): S3Storage {
  const buckets = ref<S3BucketList>({ names: null, loading: false, error: null });
  const stats = ref<Record<string, S3BucketStat>>({});
  const credentials = ref<S3PublicCredentials | null>(null);

  let seq = 0;
  let listedSinceStart = false;
  let credSeq = 0;

  const running = computed(() => service.value?.state === "running");

  async function loadStats(names: string[], at: number): Promise<void> {
    const next: Record<string, S3BucketStat> = {};
    for (const name of names) next[name] = stats.value[name] ?? "loading";
    stats.value = next;
    await Promise.all(
      names.map(async (name) => {
        let result: S3BucketStat;
        try {
          result = await bucketStats(name);
        } catch {
          result = "error";
        }
        if (seq !== at) return;
        stats.value = { ...stats.value, [name]: result };
      }),
    );
  }

  async function load(): Promise<void> {
    seq += 1;
    const at = seq;
    listedSinceStart = true;
    buckets.value = { names: buckets.value.names, loading: true, error: null };
    try {
      const names = await listBuckets();
      if (seq !== at) return;
      buckets.value = { names, loading: false, error: null };
      await loadStats(names, at);
    } catch (e) {
      if (seq !== at) return;
      listedSinceStart = false;
      buckets.value = { names: null, loading: false, error: (e as IpcError).message };
      stats.value = {};
    }
  }

  function clear(): void {
    seq += 1;
    listedSinceStart = false;
    buckets.value = { names: null, loading: false, error: null };
    stats.value = {};
  }

  watch(
    running,
    (isRunning) => {
      if (!isRunning) {
        clear();
        return;
      }
      if (!listedSinceStart) void load();
    },
    { immediate: true },
  );

  async function loadCredentials(): Promise<void> {
    credSeq += 1;
    const at = credSeq;
    try {
      const c = await s3Credentials(false);
      if (credSeq !== at) return;
      credentials.value = { endpoint: c.endpoint, access_key: c.access_key };
    } catch {
      if (credSeq !== at) return;
      credentials.value = null;
    }
  }

  const credentialsKey = computed(() => {
    const s = service.value;
    return s && isInstalled(s) ? `${s.service}:${s.port}:${s.state}` : null;
  });

  watch(
    credentialsKey,
    (key) => {
      if (key === null) {
        credSeq += 1;
        credentials.value = null;
        return;
      }
      void loadCredentials();
    },
    { immediate: true },
  );

  function reload(): void {
    if (running.value) void load();
  }

  /** Drops the bucket locally, then lists again: a listing or stats fetch
   *  already in flight may predate the drop, and the fresh listing supersedes
   *  both. */
  function bucketDropped(name: string): void {
    const names = buckets.value.names;
    if (names) buckets.value = { ...buckets.value, names: names.filter((n) => n !== name) };
    const { [name]: _gone, ...rest } = stats.value;
    stats.value = rest;
    if (running.value) void load();
  }

  const state = computed<S3StorageState>(() => ({
    buckets: buckets.value,
    stats: stats.value,
    credentials: credentials.value,
  }));

  return { state, reload, bucketDropped };
}
