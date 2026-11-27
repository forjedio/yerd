/** The attribute marking the toast viewport, so modal layers can recognise it. */
export const TOASTER_ATTR = "data-yerd-toaster";

/**
 * Whether an outside-interaction event from a reka dismissable layer started
 * inside the toast viewport. Toasts sit outside every dialog, so a click on
 * one would otherwise read as an outside click and dismiss the dialog.
 */
export function isToasterInteraction(event: Event): boolean {
  const original = (event as CustomEvent<{ originalEvent?: Event }>).detail?.originalEvent;
  const target = original?.target ?? event.target;
  return target instanceof Element && target.closest(`[${TOASTER_ATTR}]`) !== null;
}

/** An `interact-outside` handler that keeps a dialog open for toast clicks. */
export function keepOpenForToaster(event: Event): void {
  if (isToasterInteraction(event)) event.preventDefault();
}
