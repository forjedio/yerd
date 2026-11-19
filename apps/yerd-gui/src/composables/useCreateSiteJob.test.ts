import { flushPromises, mount } from "@vue/test-utils";
import { defineComponent } from "vue";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const createSite = vi.hoisted(() => vi.fn());
const jobStatus = vi.hoisted(() => vi.fn());

vi.mock("@/ipc/client", () => ({
  IpcError: class IpcError extends Error {},
  createSite,
  jobCancel: vi.fn().mockResolvedValue(undefined),
  jobStatus,
}));

vi.mock("@/composables/useToast", () => ({
  useToast: () => ({ success: vi.fn(), error: vi.fn() }),
}));

import { phaseStatus, useCreateSiteJob } from "./useCreateSiteJob";
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

function status(state: JobState) {
  return { log: [], next_cursor: 0, phase: "Scaffolding", state, error: null };
}

function mountJob() {
  let job!: ReturnType<typeof useCreateSiteJob>;
  const w = mount(
    defineComponent({
      setup() {
        job = useCreateSiteJob({ isOpen: () => true, domain: () => "blog.test", onCreated: () => {} });
        return () => null;
      },
    }),
  );
  return { w, job };
}

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
    void job.start(SPEC);
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
    await job.start(SPEC);
    expect(job.finishedAt.value).toBe(Date.now());
    job.reset();
    expect(job.startedAt.value).toBeNull();
    expect(job.finishedAt.value).toBeNull();
    w.unmount();
  });
});
