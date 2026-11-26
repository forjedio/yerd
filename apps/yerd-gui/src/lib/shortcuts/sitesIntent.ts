/**
 * A one-shot request to open a Sites-page dialog from elsewhere (the command
 * palette, a shortcut, or the system-tray "New Laravel site…" item). The
 * Link/Park/Create commands set this, then navigate to `/sites`; SitesView
 * consumes and clears it on mount (or while already mounted).
 *
 * Module-level singleton, like `useViewActions` - and per-webview, so the
 * standalone dumps/mails windows hold their own (always-null) copy.
 */
import { ref } from "vue";

export type SitesIntent = "link" | "park" | "create";

export const sitesIntent = ref<SitesIntent | null>(null);

/**
 * A one-shot request to open the Sites page on one site (the Overview's edit
 * pencil). Set it to the site name, then navigate to `/sites`; SitesView
 * clears it once its list has loaded, selecting the site if it is listed, and
 * leaving `/sites` clears it too, so an interrupted trip never fires later.
 */
export const sitesSelectIntent = ref<string | null>(null);
