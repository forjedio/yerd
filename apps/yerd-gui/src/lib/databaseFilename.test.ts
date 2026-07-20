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

  it("prefixes Windows reserved device-name stems, case-insensitively", () => {
    expect(databaseExportFilename("con")).toBe("_con.sql");
    expect(databaseExportFilename("PRN")).toBe("_PRN.sql");
    expect(databaseExportFilename("Aux")).toBe("_Aux.sql");
    expect(databaseExportFilename("nul")).toBe("_nul.sql");
    expect(databaseExportFilename("com1")).toBe("_com1.sql");
    expect(databaseExportFilename("COM9")).toBe("_COM9.sql");
    expect(databaseExportFilename("lpt1")).toBe("_lpt1.sql");
    expect(databaseExportFilename("LPT9")).toBe("_LPT9.sql");
  });

  it("guards a reserved stem even when it already carries an extension", () => {
    expect(databaseExportFilename("nul.backup")).toBe("_nul.backup.sql");
  });

  it("leaves names that merely resemble reserved devices unchanged", () => {
    expect(databaseExportFilename("console")).toBe("console.sql");
    expect(databaseExportFilename("com")).toBe("com.sql");
    expect(databaseExportFilename("com0")).toBe("com0.sql");
    expect(databaseExportFilename("comment")).toBe("comment.sql");
    expect(databaseExportFilename("lpt")).toBe("lpt.sql");
  });
});
