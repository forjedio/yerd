import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useToast } from "@/composables/useToast";

import { copyUrlWithToast, copyWithToast, toastOnError } from "./toastActions";

const writeText = vi.fn();

beforeEach(() => {
  writeText.mockReset();
  Object.assign(navigator, { clipboard: { writeText } });
});

afterEach(() => {
  const { toasts, dismiss } = useToast();
  [...toasts.value].forEach((t) => dismiss(t.id));
});

function lastToast() {
  const { toasts } = useToast();
  return toasts.value[toasts.value.length - 1];
}

describe("copyWithToast", () => {
  it("copies and raises the success toast", async () => {
    writeText.mockResolvedValue(undefined);
    expect(await copyWithToast("abc", { title: "Copied logs", detail: "Paste it" })).toBe(true);
    expect(writeText).toHaveBeenCalledWith("abc");
    expect(lastToast()).toMatchObject({ kind: "success", title: "Copied logs", detail: "Paste it" });
  });

  it("raises the blocked-clipboard error by default when the write fails", async () => {
    writeText.mockRejectedValue(new Error("denied"));
    expect(await copyWithToast("abc", { title: "Copied" })).toBe(false);
    expect(lastToast()).toMatchObject({
      kind: "error",
      title: "Couldn't copy",
      detail: "Your browser blocked clipboard access.",
    });
  });

  it("uses a caller's failure title", async () => {
    writeText.mockRejectedValue(new Error("denied"));
    await copyWithToast("abc", { title: "Copied" }, "Couldn't copy web root");
    expect(lastToast()).toMatchObject({
      kind: "error",
      title: "Couldn't copy web root",
      detail: "Your browser blocked clipboard access.",
    });
  });
});

describe("copyUrlWithToast", () => {
  const cases = [
    { ok: true, toast: { kind: "success", title: "Copied URL" } },
    {
      ok: false,
      toast: { kind: "error", title: "Couldn't copy URL", detail: "Your browser blocked clipboard access." },
    },
  ];

  for (const { ok, toast } of cases) {
    it(`toasts ${toast.title}`, async () => {
      if (ok) writeText.mockResolvedValue(undefined);
      else writeText.mockRejectedValue(new Error("denied"));
      expect(await copyUrlWithToast("https://a.test")).toBe(ok);
      expect(writeText).toHaveBeenCalledWith("https://a.test");
      expect(lastToast()).toMatchObject(toast);
    });
  }
});

describe("toastOnError", () => {
  it("resolves true and stays quiet when the action succeeds", async () => {
    expect(await toastOnError("Couldn't open site", async () => "ok")).toBe(true);
    expect(useToast().toasts.value).toHaveLength(0);
  });

  it("toasts the failure over the error's message", async () => {
    const failing = async () => {
      throw new Error("no handler");
    };
    expect(await toastOnError("Couldn't open site", failing)).toBe(false);
    expect(lastToast()).toMatchObject({ kind: "error", title: "Couldn't open site", detail: "no handler" });
  });
});
