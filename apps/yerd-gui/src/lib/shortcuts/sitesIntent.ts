/**
 * A one-shot request to open a Sites-page dialog from elsewhere (the command
 * palette / a shortcut). The Link/Park commands set this, then navigate to
 * `/sites`; SitesView consumes and clears it on mount (or while already mounted).
 *
 * Module-level singleton, like `useViewActions` - and per-webview, so the
 * standalone dumps/mails windows hold their own (always-null) copy.
 */
import { ref } from "vue";

export type SitesIntent = "link" | "park";

export const sitesIntent = ref<SitesIntent | null>(null);
