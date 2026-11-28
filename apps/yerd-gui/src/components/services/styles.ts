/** Shared classes for the Services screen's compact form fields. */

/** A 32px native text field, as the draft draws the inline add rows and filters. */
export const FIELD =
  "h-8 rounded-md border border-input bg-background px-2.5 font-mono text-xs outline-none transition-colors placeholder:font-sans placeholder:text-faint focus-visible:border-brand/60 focus-visible:ring-1 focus-visible:ring-brand/40 disabled:cursor-not-allowed disabled:opacity-50";

/** A 36px list row in the Databases and Buckets cards. */
export const ROW = "flex h-9 items-center gap-2 pl-4 pr-2";

/** A compact button inside an in-place confirm row (Cancel, Delete, Restore). */
export const CONFIRM_BTN =
  "inline-flex h-7 shrink-0 items-center rounded-md px-3 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";
