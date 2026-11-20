import { useToast } from "@/composables/useToast";

/** Toast-reporting wrappers for one-shot UI actions: clipboard copies and
 *  fire-and-forget calls (open in browser, reveal in Finder, open a window). */

/** A toast's title and optional detail line. */
export interface ToastCopy {
  title: string;
  detail?: string;
}

const BLOCKED = "Your browser blocked clipboard access.";

/**
 * Puts `text` on the clipboard and toasts the outcome: `copied` on success,
 * otherwise `failedTitle` over a blocked-clipboard detail. Resolves to whether
 * the copy succeeded.
 */
export async function copyWithToast(
  text: string,
  copied: ToastCopy,
  failedTitle = "Couldn't copy",
): Promise<boolean> {
  const toast = useToast();
  try {
    await navigator.clipboard.writeText(text);
    toast.success(copied.title, copied.detail);
    return true;
  } catch {
    toast.error(failedTitle, BLOCKED);
    return false;
  }
}

/** Copies a URL with the "Copied URL" / "Couldn't copy URL" toasts. */
export function copyUrlWithToast(url: string): Promise<boolean> {
  return copyWithToast(url, { title: "Copied URL" }, "Couldn't copy URL");
}

/**
 * Runs `action`, toasting `failure` over the error's message if it throws.
 * Resolves to whether the action succeeded.
 */
export async function toastOnError(failure: string, action: () => Promise<unknown>): Promise<boolean> {
  try {
    await action();
    return true;
  } catch (e) {
    useToast().error(failure, e instanceof Error ? e.message : String(e));
    return false;
  }
}
