/**
 * Build a portable suggested SQL export filename without changing the DB name.
 *
 * Sanitised for the strictest useful portable target (Windows): its forbidden
 * filename characters plus controls (which also cover embedded tabs/newlines)
 * become `_`, trailing dots/spaces are dropped, a reserved device-name stem
 * (`CON`, `PRN`, `AUX`, `NUL`, `COM1`-`COM9`, `LPT1`-`LPT9`, case-insensitively,
 * even with an extension) is prefixed so it can't name a device, and an empty
 * stem falls back to `database`.
 */
export function databaseExportFilename(databaseName: string): string {
  const stem =
    databaseName.replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_").replace(/[. ]+$/g, "_") || "database";
  const guarded = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])(\.|$)/i.test(stem) ? `_${stem}` : stem;
  return `${guarded}.sql`;
}
