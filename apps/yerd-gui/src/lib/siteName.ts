/**
 * Derive a valid site name from an arbitrary string (typically a chosen folder's
 * last path component), for the Link modal's auto-naming.
 *
 * A faithful mirror of the CLI/daemon's `slugify_site_name`
 * (`crates/yerd-core/src/site.rs`): lowercases, replaces every run of characters
 * outside `[a-z0-9]` (`_`, `.`, spaces, ...) with a single `-`, trims
 * leading/trailing `-`, and caps at the 63-byte DNS-label limit. Returns `null`
 * when nothing valid remains. Its output is always `[a-z0-9-]` with no
 * leading/trailing/doubled `-`, so the daemon's strict `Site::linked` validator
 * always accepts it. Keep the two in step.
 *
 * `for..of` iterates Unicode code points, matching the Rust `raw.chars()` loop,
 * and the 63-cap is applied to the already-ASCII output so `.length` counts
 * bytes.
 */
export function slugifySiteName(raw: string): string | null {
  let out = "";
  for (const ch of raw) {
    if (isAsciiAlphanumeric(ch)) {
      out += ch.toLowerCase();
    } else if (out.length > 0 && !out.endsWith("-")) {
      out += "-";
    }
  }
  while (out.endsWith("-")) out = out.slice(0, -1);
  if (out === "") return null;
  if (out.length > 63) {
    out = out.slice(0, 63);
    while (out.endsWith("-")) out = out.slice(0, -1);
  }
  return out === "" ? null : out;
}

function isAsciiAlphanumeric(ch: string): boolean {
  if (ch.length !== 1) return false;
  return (ch >= "0" && ch <= "9") || (ch >= "a" && ch <= "z") || (ch >= "A" && ch <= "Z");
}
