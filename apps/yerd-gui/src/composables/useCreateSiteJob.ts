import { computed, onUnmounted, ref } from "vue";

import { useOperations } from "@/composables/useOperations";
import { invalidate } from "@/composables/useResource";
import type { StarterId } from "@/composables/useSiteStarters";
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

/**
 * What the dialog needs to redraw a create it didn't start: a remounted New
 * site dialog restores its form from this and shows the job's progress.
 */
export interface CreateSiteSnapshot {
  /** The starter the create was started from. */
  starter: StarterId;
  name: string;
  location: string;
  php: string;
  secure: boolean;
  /** The site's `name.tld`, for the title and the finish toast. */
  domain: string;
  /** The Done screen's one-line summary, as it read when the create started. */
  doneSummary: string;
}

export interface CreateSiteJobOptions {
  /** Whether the dialog showing the job is open (a finish it shows isn't toasted). */
  isOpen: () => boolean;
  /** Called when a create succeeds while this dialog is mounted. */
  onCreated: () => void;
}

const OPERATION_ID = "site-create";

const state = ref<JobState>("running");
const phase = ref("Starting");
const log = ref<string[]>([]);
const error = ref<string | null>(null);
const cancelRequested = ref(false);
const started = ref(false);
const jobId = ref<string | null>(null);
const startedAt = ref<number | null>(null);
const finishedAt = ref<number | null>(null);
const snapshot = ref<CreateSiteSnapshot | null>(null);
let cursor = 0;
let pollTimer: ReturnType<typeof setTimeout> | null = null;
let generation = 0;
let viewer: CreateSiteJobOptions | null = null;

const inFlight = computed(() => started.value && state.value === "running");

function stop(): void {
  if (pollTimer !== null) {
    clearTimeout(pollTimer);
    pollTimer = null;
  }
}

function reset(): void {
  if (inFlight.value) return;
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
  startedAt.value = null;
  finishedAt.value = null;
  snapshot.value = null;
}

/** Record the end of the job: stamp it, refresh the site list, and toast the
 *  outcome unless an open dialog is already showing it. */
function finish(final: JobState, message: string | null): void {
  if (finishedAt.value === null) finishedAt.value = Date.now();
  useOperations().end(OPERATION_ID);
  const domain = snapshot.value?.domain ?? "the site";
  if (final === "succeeded") {
    if (viewer) viewer.onCreated();
    else void invalidate("sites");
  }
  if (viewer?.isOpen()) return;
  const toast = useToast();
  if (final === "succeeded") toast.success(`${domain} is ready`);
  else if (final === "failed") toast.error(`Couldn't create ${domain}`, message ?? "creation failed");
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
        useOperations().update(OPERATION_ID, { detail: r.phase });
        pollTimer = setTimeout(poll, 600);
        return;
      }
      finish(r.state, r.error);
    } catch (e) {
      if (jobId.value !== id) return;
      state.value = "failed";
      error.value = (e as IpcError).message;
      finish("failed", error.value);
    }
  })();
}

/** Start a create. Only one runs at a time: a start while one is in flight is ignored. */
async function start(spec: CreateSiteSpec, snap: CreateSiteSnapshot): Promise<void> {
  if (inFlight.value) return;
  reset();
  const gen = generation;
  started.value = true;
  startedAt.value = Date.now();
  snapshot.value = snap;
  useOperations().begin({ id: OPERATION_ID, kind: "site-create", label: `Creating ${snap.domain}` });
  try {
    const id = await createSite(spec);
    if (gen !== generation) return;
    jobId.value = id;
    poll();
  } catch (e) {
    if (gen !== generation) return;
    state.value = "failed";
    error.value = (e as IpcError).message;
    finish("failed", error.value);
  }
}

async function cancel(): Promise<void> {
  if (!jobId.value || cancelRequested.value) return;
  cancelRequested.value = true;
  await jobCancel(jobId.value).catch(() => undefined);
}

/**
 * The app's one site-create job: start it, poll its progress and log, cancel
 * it. State is module-level, like `useOperations`, so the job outlives the New
 * site dialog and the Sites view: polling carries on after either unmounts, a
 * finish nobody is watching is toasted (and refreshes the site list), and a
 * remounted dialog picks the running job back up from `snapshot`.
 *
 * `opts` registers the calling dialog as the job's viewer until it unmounts.
 */
export function useCreateSiteJob(opts: CreateSiteJobOptions) {
  viewer = opts;
  onUnmounted(() => {
    if (viewer === opts) viewer = null;
  });

  return {
    state,
    phase,
    log,
    error,
    cancelRequested,
    /** True from `start()` until the job ends, including before the daemon returns its id. */
    inFlight,
    /** Epoch ms when `start()` ran, for the elapsed clock; null before a start. */
    startedAt,
    /** Epoch ms when the job was first seen finished (any terminal state). */
    finishedAt,
    snapshot,
    start,
    cancel,
    reset,
  };
}

/** Test-only: forget any job so specs start from a clean slate. */
export function resetCreateSiteJob(): void {
  stop();
  generation += 1;
  started.value = false;
  state.value = "running";
  reset();
  viewer = null;
}
