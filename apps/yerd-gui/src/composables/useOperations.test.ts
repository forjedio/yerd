import { afterEach, describe, expect, it } from "vitest";

import { useOperations } from "./useOperations";

// The registry is a module-level singleton; clear it after each test so cases
// don't leak operations into one another.
afterEach(() => {
  const { active, end } = useOperations();
  [...active.value].forEach((o) => end(o.id));
});

describe("useOperations", () => {
  it("begins, reports, and ends an operation", () => {
    const ops = useOperations();
    expect(ops.any.value).toBe(false);

    ops.begin({ id: "php-install:8.3", kind: "php-install", label: "Installing PHP 8.3" });
    expect(ops.any.value).toBe(true);
    expect(ops.isRunning("php-install:8.3")).toBe(true);
    expect(ops.get("php-install:8.3")?.label).toBe("Installing PHP 8.3");

    ops.end("php-install:8.3");
    expect(ops.isRunning("php-install:8.3")).toBe(false);
    expect(ops.any.value).toBe(false);
  });

  it("upserts by id rather than duplicating", () => {
    const ops = useOperations();
    ops.begin({ id: "daemon", kind: "daemon-start", label: "Starting Yerd" });
    ops.begin({ id: "daemon", kind: "daemon-start", label: "Starting Yerd again" });

    expect(ops.active.value.filter((o) => o.id === "daemon")).toHaveLength(1);
    expect(ops.get("daemon")?.label).toBe("Starting Yerd again");
  });

  it("update patches a live operation and ignores an ended one", () => {
    const ops = useOperations();
    ops.begin({ id: "php-install:8.4", kind: "php-install", label: "Installing PHP 8.4" });
    ops.update("php-install:8.4", { detail: "downloading…" });
    expect(ops.get("php-install:8.4")?.detail).toBe("downloading…");

    ops.end("php-install:8.4");
    ops.update("php-install:8.4", { detail: "late" });
    expect(ops.isRunning("php-install:8.4")).toBe(false); // not revived
  });

  it("shares state across separate useOperations() calls (singleton)", () => {
    const a = useOperations();
    const b = useOperations();
    a.begin({ id: "site-create:blog", kind: "site-create", label: "Creating blog" });
    expect(b.isRunning("site-create:blog")).toBe(true);
  });
});
