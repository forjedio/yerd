import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const createSite = vi.hoisted(() => vi.fn());
const jobStatus = vi.hoisted(() => vi.fn());
const toastSuccess = vi.hoisted(() => vi.fn());
const toastError = vi.hoisted(() => vi.fn());
const invalidate = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  createSite,
  jobCancel: vi.fn().mockResolvedValue(undefined),
  jobStatus,
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: toastSuccess, error: toastError }),
}));

vi.mock("@/composables/useResource", () => ({ invalidate }));

import { useOperations } from "./useOperations";
import {
  type CreateSiteSnapshot,
  phaseStatus,
  resetCreateSiteJob,
  useCreateSiteJob,
} from "./useCreateSiteJob";
import type { CreateSiteSpec, JobState } from "@/ipc/types";

const PHASES = ["Preflight", "Scaffolding", "Registering", "Done"];

describe("phaseStatus", () => {
  it.each<[string, JobState, string, string]>([
    ["Scaffolding", "running", "Preflight", "done"],
    ["Scaffolding", "running", "Scaffolding", "active"],
    ["Scaffolding", "running", "Registering", "todo"],
    ["Registering", "succeeded", "Done", "done"],
    ["Starting", "running", "Preflight", "active"],
    ["Installing Node", "running", "Scaffolding", "todo"],
    ["Scaffolding", "failed", "Preflight", "done"],
    ["Scaffolding", "failed", "Scaffolding", "todo"],
    ["Scaffolding", "cancelled", "Scaffolding", "todo"],
    ["Starting", "failed", "Preflight", "todo"],
  ])("phase %s while %s marks %s as %s", (current, state, p, expected) => {
    expect(phaseStatus(PHASES, current, state, p)).toBe(expected);
  });
});

const SPEC: CreateSiteSpec = {
  name: "blog",
  parent_dir: "/srv",
  php: "8.4",
  secure: false,
  framework: { framework: "slim" },
};

const SNAP: CreateSiteSnapshot = {
  starter: "slim",
  name: "blog",
  location: "/srv",
  php: "8.4",
  secure: false,
  domain: "blog.test",
  doneSummary: "Slim · PHP 8.4",
};

function status(state: JobState) {
  return { log: [], next_cursor: 0, phase: "Scaffolding", state, error: null };
}

function mountJob(isOpen = true, onCreated: () => void = () => {}) {
  let job!: ReturnType<typeof useCreateSiteJob>;
  const w = mount(
    defineComponent({
      setup() {
        job = useCreateSiteJob({ isOpen: () => isOpen, onCreated });
        return () => null;
      },
    }),
  );
  return { w, job };
}

beforeEach(() => {
  resetCreateSiteJob();
  toastSuccess.mockReset();
  toastError.mockReset();
  invalidate.mockReset();
  const ops = useOperations();
  ops.active.value.forEach((o) => ops.end(o.id));
});

describe("useCreateSiteJob timing", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-01-01T00:00:00Z"));
    createSite.mockReset().mockResolvedValue("job-1");
    jobStatus.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("records when the job started and when it was first seen finished", async () => {
    jobStatus.mockResolvedValueOnce(status("running")).mockResolvedValue(status("succeeded"));
    const { w, job } = mountJob();
    expect(job.startedAt.value).toBeNull();
    const start = Date.now();
    void job.start(SPEC, SNAP);
    await flushPromises();
    expect(job.startedAt.value).toBe(start);
    expect(job.finishedAt.value).toBeNull();
    await vi.advanceTimersByTimeAsync(72_000);
    await flushPromises();
    expect(job.state.value).toBe("succeeded");
    expect(job.finishedAt.value).toBe(start + 600);
    w.unmount();
  });

  it("stamps the finish of a create that fails to start, and clears both on reset", async () => {
    createSite.mockRejectedValue(new Error("busy"));
    const { w, job } = mountJob();
    await job.start(SPEC, SNAP);
    expect(job.finishedAt.value).toBe(Date.now());
    job.reset();
    expect(job.startedAt.value).toBeNull();
    expect(job.finishedAt.value).toBeNull();
    w.unmount();
  });
});

describe("useCreateSiteJob lifetime", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    createSite.mockReset().mockResolvedValue("job-1");
    jobStatus.mockReset();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("keeps polling after its dialog unmounts, then toasts and refreshes the sites", async () => {
    jobStatus.mockResolvedValueOnce(status("running")).mockResolvedValue(status("succeeded"));
    const onCreated = vi.fn();
    const { w, job } = mountJob(true, onCreated);
    void job.start(SPEC, SNAP);
    await flushPromises();
    expect(useOperations().isRunning("site-create")).toBe(true);
    w.unmount();
    await vi.advanceTimersByTimeAsync(600);
    await flushPromises();
    expect(job.state.value).toBe("succeeded");
    expect(onCreated).not.toHaveBeenCalled();
    expect(invalidate).toHaveBeenCalledWith("sites");
    expect(toastSuccess).toHaveBeenCalledWith("blog.test is ready");
    expect(useOperations().isRunning("site-create")).toBe(false);
  });

  it("toasts a failure nobody is watching", async () => {
    jobStatus.mockResolvedValue({ ...status("failed"), error: "composer exploded" });
    const { w, job } = mountJob(false);
    void job.start(SPEC, SNAP);
    await flushPromises();
    expect(toastError).toHaveBeenCalledWith("Couldn't create blog.test", "composer exploded");
    w.unmount();
  });

  it("shares one job across mounts and refuses a second start while it runs", async () => {
    jobStatus.mockResolvedValue(status("running"));
    const first = mountJob();
    void first.job.start(SPEC, SNAP);
    await flushPromises();
    first.w.unmount();
    const second = mountJob();
    expect(second.job.inFlight.value).toBe(true);
    expect(second.job.snapshot.value).toEqual(SNAP);
    await second.job.start({ ...SPEC, name: "other" }, { ...SNAP, name: "other" });
    second.job.reset();
    expect(createSite).toHaveBeenCalledTimes(1);
    expect(second.job.snapshot.value?.name).toBe("blog");
    second.w.unmount();
  });
});
