import { computed, ref, watch } from "vue";

import { loadIdes, useIdes } from "@/composables/useIdes";
import { useToast } from "@/composables/useToast";
import {
  getPreferredIde,
  getSiteIdeOverrides,
  IpcError,
  openInIde,
  openInSystemDefault,
  setSiteIdeOverride,
} from "@/ipc/client";
import type { SiteEntry } from "@/ipc/types";
import { resolveIde, SYSTEM_LABEL } from "@/lib/ideChoice";

/**
 * The per-site editor choice the Sites inspector shows: the global preference,
 * this site's override, what the Editor button resolves to, the options of the
 * Editor submenu, and the actions to open the site and to change its override.
 * `site` returns the site in view, or `null` while nothing is shown; every change
 * of it drops the previous site's state before loading the next.
 */
export function useSiteEditor(site: () => SiteEntry | null) {
  const toast = useToast();
  const { installedIdes } = useIdes();
  const globalIde = ref<string | null>(null);
  const siteIdeOverride = ref<string | null>(null);
  let editorPreferenceRequestId = 0;

  /** What this site's editor button resolves to right now: its own override,
   *  else the global preference, else the best-ranked detected editor, else the
   *  folder. */
  const editorChoice = computed(() =>
    resolveIde(siteIdeOverride.value, globalIde.value, installedIdes.value),
  );

  const editorLabel = computed(() => editorChoice.value.label);

  const editorTooltip = computed(() =>
    editorChoice.value.kind === "system"
      ? "Open the site folder"
      : `Open the site folder in ${editorChoice.value.label}`,
  );

  const ideOptions = computed(() => [
    {
      value: "default",
      label: `Use default (${resolveIde(null, globalIde.value, installedIdes.value).label})`,
    },
    ...installedIdes.value.map((ide) => ({ value: ide.id, label: ide.label })),
    { value: "system", label: SYSTEM_LABEL },
  ]);

  /** An override naming an editor that isn't installed here shows as the
   *  default entry while the stored preference is left untouched. */
  const selectedIde = computed(() => {
    const stored = siteIdeOverride.value ?? "default";
    return ideOptions.value.some((option) => option.value === stored) ? stored : "default";
  });

  /** Read the global preference and this site's override. The request id guards
   *  against a fast site switch resolving out of order and applying one site's
   *  override to another. */
  async function loadEditorPreferences(siteName: string): Promise<void> {
    const requestId = ++editorPreferenceRequestId;
    try {
      const [global, overrides] = await Promise.all([getPreferredIde(), getSiteIdeOverrides()]);
      if (requestId !== editorPreferenceRequestId) return;
      globalIde.value = global;
      siteIdeOverride.value = overrides[siteName] ?? null;
    } catch (error) {
      if (requestId !== editorPreferenceRequestId) return;
      globalIde.value = null;
      siteIdeOverride.value = null;
      toast.error("Couldn't load the editor preference", (error as IpcError).message);
    }
  }

  /** Store this site's override, showing it optimistically. The rollback on
   *  failure is guarded by the same request id (and the site now in view) as
   *  `loadEditorPreferences`: a write that rejects after the user has moved on
   *  must not stamp the old site's value onto the new one. */
  async function changeIde(target: SiteEntry, value: string): Promise<void> {
    const requestId = editorPreferenceRequestId;
    const previous = siteIdeOverride.value;
    const next = value === "default" ? null : value;
    siteIdeOverride.value = next;
    try {
      await setSiteIdeOverride(target.name, next);
    } catch (error) {
      if (requestId === editorPreferenceRequestId && site()?.name === target.name) {
        siteIdeOverride.value = previous;
      }
      toast.error("Couldn't change the editor", (error as IpcError).message);
    }
  }

  async function openEditor(target: SiteEntry): Promise<void> {
    try {
      const choice = editorChoice.value;
      if (choice.kind === "system") {
        await openInSystemDefault(target.name);
      } else {
        await openInIde(target.name, choice.id);
      }
    } catch (error) {
      toast.error("Couldn't open the site folder", (error as IpcError).message);
    }
  }

  /** The request id is invalidated and the previous site's state cleared in the
   *  same tick: leaving it in place would let the Editor button open the new
   *  site's folder in the old site's editor until the host calls resolve. */
  watch(
    () => site()?.name,
    (name) => {
      editorPreferenceRequestId += 1;
      globalIde.value = null;
      siteIdeOverride.value = null;
      if (!name) return;
      void loadIdes();
      void loadEditorPreferences(name);
    },
    { immediate: true },
  );

  return {
    globalIde,
    siteIdeOverride,
    editorChoice,
    editorLabel,
    editorTooltip,
    ideOptions,
    selectedIde,
    changeIde,
    openEditor,
  };
}

/**
 * Open `siteName` in its resolved editor without holding any state: used by the
 * list's ⌘E and the New site dialog's Done screen. Resolves the same chain as
 * the inspector (site override, global preference, best detected editor, the
 * system default) and toasts on failure.
 */
export async function openSiteInEditor(siteName: string): Promise<void> {
  const toast = useToast();
  const { installedIdes } = useIdes();
  try {
    await loadIdes();
    const [global, overrides] = await Promise.all([getPreferredIde(), getSiteIdeOverrides()]);
    const choice = resolveIde(overrides[siteName] ?? null, global, installedIdes.value);
    if (choice.kind === "system") await openInSystemDefault(siteName);
    else await openInIde(siteName, choice.id);
  } catch (error) {
    toast.error("Couldn't open the site folder", (error as IpcError).message);
  }
}
