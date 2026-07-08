/**
 * Client-side domain *shape* validation for the Manage-domains UI.
 *
 * A faithful, deliberately lenient mirror of the CLI's `validate_domain`
 * (`bin/yerd/src/map.rs`): it catches obvious typos before a round-trip but is
 * NOT authoritative. The daemon still enforces the strict rules the CLI leaves to
 * it - leading/trailing hyphens, a bare `*`, and TLD membership - and its errors
 * surface to the user via the failed IPC call's toast. Keep the two in step.
 */

/** A shape error message for `fqdn`, or `null` when the shape is acceptable.
 *  Mirrors `bin/yerd/src/map.rs::validate_domain`. */
export function validateDomainShape(fqdn: string): string | null {
  if (fqdn === "") return "must not be empty";
  const lowered = fqdn.toLowerCase();
  // The CLI strips one trailing dot before splitting, so `foo.test.` is valid.
  const trimmed = lowered.endsWith(".") ? lowered.slice(0, -1) : lowered;
  const labels = trimmed.split(".");
  if (labels.length < 2) return "must be a full domain including the TLD";
  for (let i = 0; i < labels.length; i++) {
    const label = labels[i];
    if (label === "") return "contains an empty label";
    if (label === "*") {
      if (i !== 0) return "'*' is only allowed as the leftmost label";
      continue;
    }
    if (!/^[a-z0-9-]+$/.test(label)) {
      return "labels may only contain [a-z0-9-] (or a leading '*')";
    }
  }
  return null;
}

/** True when `fqdn` is not under `tld` (drives the non-blocking TLD hint - the
 *  daemon is authoritative and rejects a wrong-TLD domain with `NotUnderTld`).
 *  Tolerates one trailing dot, matching {@link validateDomainShape}. */
export function isUnderTld(fqdn: string, tld: string): boolean {
  const lowered = fqdn.toLowerCase();
  const trimmed = lowered.endsWith(".") ? lowered.slice(0, -1) : lowered;
  return trimmed.endsWith(`.${tld.toLowerCase()}`);
}
