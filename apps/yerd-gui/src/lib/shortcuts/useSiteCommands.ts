/**
 * Dynamic command-palette entries for the user's sites: per site an "Open
 * {domain}" and a "Secure/Unsecure {domain}" action, grouped by domain and
 * ordered by name descending. These carry no key chord and are appended after
 * the static commands, so they sit at the bottom of the palette.
 *
 * The list is fetched lazily when the palette opens (sites aren't in the daemon
 * report, only counts), and falls back to empty when the daemon is unreachable.
 *
 * The command list keys off the `tld` string rather than the whole report
 * object: the daemon poll replaces the report by identity every few seconds, so
 * depending on it directly would churn the list and reset the palette selection
 * while it is open.
 */
import { computed, watch, type Ref } from "vue";

import { useDaemon } from "@/composables/useDaemon";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { IpcError, openInBrowser, setSecure, sitesAndParked } from "@/ipc/client";
import { siteUrl } from "@/lib/siteUrl";
import type { Site } from "@/ipc/types";
import type { Command } from "./registry";

export interface SiteCommandHandlers {
  onOpen: (site: Site) => void;
  onToggleSecure: (site: Site) => void;
}

/** Pure: build the per-site palette commands. Sorted by name descending. */
export function buildSiteCommands(
  sites: Site[],
  tld: string,
  handlers: SiteCommandHandlers,
): Command[] {
  const ordered = [...sites].sort((a, b) => b.name.localeCompare(a.name));
  return ordered.flatMap((s): Command[] => {
    const domain = `${s.name}.${tld}`;
    return [
      {
        id: `site-open:${s.name}`,
        title: `Open ${domain}`,
        group: domain,
        scopes: ["main"],
        inPalette: true,
        run: () => handlers.onOpen(s),
      },
      {
        id: `site-secure:${s.name}`,
        title: `${s.secure ? "Unsecure" : "Secure"} ${domain}`,
        group: domain,
        scopes: ["main"],
        inPalette: true,
        run: () => handlers.onToggleSecure(s),
      },
    ];
  });
}

/** Live per-site palette commands; revalidates each time the palette opens. */
export function useSiteCommands(paletteOpen: Ref<boolean>): Ref<Command[]> {
  const { report, refresh } = useDaemon();
  const toast = useToast();
  // Shared "sites" cache (same key + fetcher as the Sites view and Overview), so
  // the palette renders from the same data with no clear-then-load flash on open.
  // immediate:false: the palette is always mounted, so only fetch when it opens.
  const { data, refresh: reloadSites } = useResource("sites", sitesAndParked, {
    immediate: false,
  });
  const sites = computed(() => data.value?.sites ?? []);

  watch(paletteOpen, (open) => {
    if (open) void reloadSites();
  });

  const handlers: SiteCommandHandlers = {
    onOpen: (s) => void openInBrowser(siteUrl(s, report.value)),
    onToggleSecure: async (s) => {
      try {
        await setSecure(s.name, !s.secure);
        toast.success(`Updated ${s.name}`);
        // Invalidate the shared cache so the Sites view / Overview reflect it too.
        await Promise.all([refresh(), reloadSites()]);
      } catch (e) {
        toast.error("Couldn't update site", (e as IpcError).message);
      }
    },
  };

  const tld = computed(() => report.value?.tld ?? "test");
  return computed(() => buildSiteCommands(sites.value, tld.value, handlers));
}
