/** Build a portable suggested SQL export filename without changing the DB name. */
export function databaseExportFilename(databaseName: string): string {
  // Windows' forbidden filename characters are the strictest useful portable set;
  // controls also cover embedded tabs/newlines. Avoid trailing dots/spaces too.
  const safe = databaseName
    .replace(/[<>:"/\\|?*\u0000-\u001f]/g, "_")
    .replace(/[. ]+$/g, "_");
  return `${safe || "database"}.sql`;
}
