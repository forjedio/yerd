import { describe, expect, it } from "vitest";
import { databaseExportFilename } from "./databaseFilename";

describe("databaseExportFilename", () => {
  it("preserves safe database names", () => {
    expect(databaseExportFilename("sales-data.日本語")).toBe("sales-data.日本語.sql");
  });

  it("replaces path separators and portable filename-invalid characters", () => {
    expect(databaseExportFilename('a/b\\c:"d*?\n')).toBe("a_b_c__d___.sql");
  });

  it("never suggests an empty filename stem", () => {
    expect(databaseExportFilename("...")).toBe("_.sql");
    expect(databaseExportFilename("\n")).toBe("_.sql");
  });
});
