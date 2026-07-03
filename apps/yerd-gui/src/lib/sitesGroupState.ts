/**
 * Remembered collapse state for the Sites view's group sections.
 *
 * Pure view cosmetics (which groups are folded), so it lives in `localStorage`
 * rather than daemon config, mirroring the `theme.ts` preference pattern. The
 * Sites view only exists in the main window, so no cross-window sync is needed.
 *
 * Stored as a JSON array of the collapsed group names under one key. Deleted
 * group names may linger harmlessly (a recreated same-name group starts
 * collapsed) - the same staleness tolerance as the daemon's membership map.
 */
import { ref } from "vue";

const STORAGE_KEY = "yerd.sites.collapsedGroups";

/** The set of collapsed group names (reactive, persisted). */
const collapsed = ref<Set<string>>(load());

function load(): Set<string> {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (raw) {
      const parsed: unknown = JSON.parse(raw);
      if (Array.isArray(parsed)) return new Set(parsed.filter((v): v is string => typeof v === "string"));
    }
  } catch {
    // localStorage unavailable or malformed (e.g. unit env) - start empty.
  }
  return new Set();
}

function persist(): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify([...collapsed.value]));
  } catch {
    // Best-effort persistence; the in-memory set still applies this session.
  }
}

/** Reactive accessor for the Sites view. */
export function useSitesGroupState() {
  function isCollapsed(name: string): boolean {
    return collapsed.value.has(name);
  }

  function setCollapsed(name: string, value: boolean): void {
    const next = new Set(collapsed.value);
    if (value) next.add(name);
    else next.delete(name);
    collapsed.value = next;
    persist();
  }

  function toggle(name: string): void {
    setCollapsed(name, !isCollapsed(name));
  }

  /** Carry a renamed group's collapsed state across to its new name. */
  function rename(from: string, to: string): void {
    if (!collapsed.value.has(from)) return;
    const next = new Set(collapsed.value);
    next.delete(from);
    next.add(to);
    collapsed.value = next;
    persist();
  }

  return { collapsed, isCollapsed, setCollapsed, toggle, rename };
}

/** Test-only: reset the in-memory set (specs start from a clean slate). */
export function resetSitesGroupState(): void {
  collapsed.value = new Set();
}
