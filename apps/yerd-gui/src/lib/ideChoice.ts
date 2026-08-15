import type { IdeOption } from "@/ipc/types";

/** What an "open in editor" click should actually do. */
export type IdeChoice =
  | { kind: "ide"; id: string; label: string }
  | { kind: "system"; label: string };

/** Sentinel stored in `preferred_ide` / a per-site override for "open the
 *  folder with the desktop's default handler". */
const SYSTEM = "system";

/** Button/tooltip wording for the system arm; also the name shown by the
 *  sidebar's `Use default (…)` option when nothing is detected. */
const SYSTEM_LABEL = "Open folder";

/**
 * Resolve the preference chain: per-site override, then the global preference,
 * then the best-ranked detected editor, then the system default.
 *
 * A stored id that names an editor which is not installed here (uninstalled
 * since, or a preference written on another machine) falls through silently to
 * the next link rather than failing the click.
 */
export function resolveIde(
  override: string | null,
  global: string | null,
  detected: IdeOption[],
): IdeChoice {
  return stored(override, detected) ?? stored(global, detected) ?? autoDetected(detected);
}

function stored(value: string | null, detected: IdeOption[]): IdeChoice | null {
  if (value === SYSTEM) return { kind: "system", label: SYSTEM_LABEL };
  const match = detected.find((ide) => ide.id === value);
  return match ? { kind: "ide", id: match.id, label: match.label } : null;
}

function autoDetected(detected: IdeOption[]): IdeChoice {
  const best = detected[0];
  return best
    ? { kind: "ide", id: best.id, label: best.label }
    : { kind: "system", label: SYSTEM_LABEL };
}
