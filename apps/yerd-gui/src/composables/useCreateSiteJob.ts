import { computed, onUnmounted, ref } from "vue";

import { useToast } from "@/composables/useToast";
import { createSite, IpcError, jobCancel, jobStatus } from "@/ipc/client";
import type { CreateSiteSpec, JobState } from "@/ipc/types";

export type PhaseStatus = "done" | "active" | "todo";

/**
 * Where phase `p` stands in a create job's stepper. A phase the daemon reports
 * outside `phases` (e.g. "Installing Node") keeps the first phase active, and
 * nothing spins once the job has failed or been cancelled.
 */
export function phaseStatus(
  phases: readonly string[],
  current: string,
  state: JobState,
  p: string,
): PhaseStatus {
  if (state === "succeeded") return "done";
  const running = state === "running";
  const ci = phases.indexOf(current);
  const pi = phases.indexOf(p);
  if (ci === -1) return pi === 0 && running ? "active" : "todo";
  if (pi < ci) return "done";
  if (pi === ci && running) return "active";
  return "todo";
}

export interface CreateSiteJobOptions {
  isOpen: () => boolean;
  domain: () => string;
  onCreated: () => void;
}

/**
 * A site-create job: start it, poll its progress and log, cancel it. Polling
 * carries on while the wizard is closed so the site list still refreshes when
 * the job lands, and a job that finishes unseen is announced with a toast. It
 * stops for good once the owning component unmounts.
 */
export function useCreateSiteJob(opts: CreateSiteJobOptions) {
  const toast = useToast();

  const state = ref<JobState>("running");
  const phase = ref("Starting");
  const log = ref<string[]>([]);
  const error = ref<string | null>(null);
  const cancelRequested = ref(false);
  const started = ref(false);
  const jobId = ref<string | null>(null);
  let cursor = 0;
  let pollTimer: number | null = null;
  let unmounted = false;
  let generation = 0;

  /** True from `start()` until the job ends, including before the daemon returns its id. */
  const inFlight = computed(() => started.value && state.value === "running");

  function stop(): void {
    if (pollTimer !== null) {
      window.clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  function reset(): void {
    stop();
    generation += 1;
    started.value = false;
    jobId.value = null;
    cursor = 0;
    state.value = "running";
    phase.value = "Starting";
    log.value = [];
    error.value = null;
    cancelRequested.value = false;
  }

  function announceUnseen(final: JobState, message: string | null): void {
    if (final === "succeeded") toast.success(`${opts.domain()} is ready`);
    else if (final === "failed") {
      toast.error(`Couldn't create ${opts.domain()}`, message ?? "creation failed");
    }
  }

  function poll(): void {
    const id = jobId.value;
    if (!id) return;
    void (async () => {
      try {
        const r = await jobStatus(id, cursor);
        if (jobId.value !== id) return;
        log.value.push(...r.log);
        cursor = r.next_cursor;
        phase.value = r.phase;
        state.value = r.state;
        error.value = r.error;
        if (r.state === "running") {
          if (!unmounted) pollTimer = window.setTimeout(poll, 600);
          return;
        }
        if (r.state === "succeeded") opts.onCreated();
        if (!opts.isOpen()) announceUnseen(r.state, r.error);
      } catch (e) {
        if (jobId.value !== id) return;
        state.value = "failed";
        error.value = (e as IpcError).message;
        if (!opts.isOpen()) announceUnseen("failed", error.value);
      }
    })();
  }

  async function start(spec: CreateSiteSpec): Promise<void> {
    reset();
    const gen = generation;
    started.value = true;
    try {
      const id = await createSite(spec);
      if (gen !== generation || unmounted) return;
      jobId.value = id;
      poll();
    } catch (e) {
      if (gen !== generation) return;
      state.value = "failed";
      error.value = (e as IpcError).message;
      if (!opts.isOpen()) announceUnseen("failed", error.value);
    }
  }

  async function cancel(): Promise<void> {
    if (!jobId.value || cancelRequested.value) return;
    cancelRequested.value = true;
    await jobCancel(jobId.value).catch(() => undefined);
  }

  onUnmounted(() => {
    unmounted = true;
    stop();
  });

  return { state, phase, log, error, cancelRequested, inFlight, start, cancel, reset, stop };
}
