import { describe, expect, it } from "vitest";

import { phaseStatus } from "./useCreateSiteJob";
import type { JobState } from "@/ipc/types";

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
