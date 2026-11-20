import { mintWordPressLoginToken, openInBrowser } from "@/ipc/client";
import { isUnbound, wpAdminLoginUrl, wpAdminUrl } from "@/lib/siteUrl";
import type { SiteEntry, StatusReport } from "@/ipc/types";

/**
 * Open a site's WP Admin: one-click, pre-authenticated login when the site has
 * auto-login enabled and unbound/resolver-off isn't in the way, falling back to
 * the plain (not signed-in) link otherwise - including if minting a token fails
 * for any reason (site disappeared, daemon error). Never blocks or surfaces an
 * error, just silently degrades.
 *
 * Used by the Sites inspector's `···` menu (WP Admin); the New site dialog's
 * Done screen has its own variant for a site that isn't in the list yet.
 */
export async function openWpAdmin(
  site: SiteEntry,
  report: StatusReport | null | undefined,
): Promise<void> {
  if (!isUnbound(report) && site.wp_auto_login) {
    try {
      const token = await mintWordPressLoginToken(site.name);
      await openInBrowser(wpAdminLoginUrl(site, report, token));
      return;
    } catch {
      /* fall through to the plain link below */
    }
  }
  try {
    await openInBrowser(wpAdminUrl(site, report));
  } catch {
    /* nothing left to fall back to */
  }
}
