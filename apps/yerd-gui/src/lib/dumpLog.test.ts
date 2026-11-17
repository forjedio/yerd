import { describe, expect, it } from "vitest";

import { logRowBody } from "./dumpLog";

describe("logRowBody", () => {
  it.each<[string, Record<string, unknown>, string]>([
    ["no context key", { level: "info", message: "hello" }, "info hello"],
    ["null context", { level: "info", message: "hello", context: null }, "info hello"],
    ["empty object context", { level: "info", message: "hello", context: {} }, "info hello"],
    ["empty array context", { level: "info", message: "hello", context: [] }, "info hello"],
    ["empty string context", { level: "info", message: "hello", context: "" }, "info hello"],
    [
      "flat object context",
      { level: "info", message: "hello", context: { user: 1 } },
      'info hello\n{\n  "user": 1\n}',
    ],
    [
      "nested context",
      { level: "error", message: "boom", context: { user: { id: 1 }, tags: ["a"] } },
      'error boom\n{\n  "user": {\n    "id": 1\n  },\n  "tags": [\n    "a"\n  ]\n}',
    ],
    [
      "list context",
      { level: "info", message: "hello", context: ["a"] },
      'info hello\n[\n  "a"\n]',
    ],
    ["context without level or message", { context: { user: 1 } }, '{\n  "user": 1\n}'],
    ["missing level", { message: "hello" }, "hello"],
    ["string scalar context", { level: "info", message: "hello", context: "x" }, 'info hello\n"x"'],
    ["numeric scalar context", { level: "info", message: "hello", context: 0 }, "info hello\n0"],
  ])("%s", (_name, payload, expected) => {
    expect(logRowBody(payload)).toBe(expected);
  });
});
