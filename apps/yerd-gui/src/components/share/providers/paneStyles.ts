/**
 * The Tunnel providers sheet's shared class strings, as drawn in the design:
 * sections stacked 18px apart, each a small caps label over a bordered box
 * and an optional help line.
 */

/** A pane's section stack. */
export const PANE = "flex flex-col gap-[18px]";

/** One section: label, content, help. */
export const SECTION = "flex flex-col gap-2";

/** A section's small caps label. */
export const LABEL = "text-[10.5px] font-semibold uppercase tracking-[0.84px] text-faint";

/** The bordered box a section's state sits in. */
export const BOX = "flex items-center gap-3 rounded-[9px] border bg-background px-3.5 py-[11px]";

/** A section's help line. */
export const HELP = "text-[11.5px] leading-normal text-muted-foreground";

/** A status chip (Ready, Installed, Connected...). */
export const CHIP =
  "inline-flex h-[19px] shrink-0 items-center gap-1.5 rounded-[5px] px-[7px] text-[10.5px] font-semibold uppercase tracking-[0.53px]";

/** A text-only action (Remove token, Restart, an external link). */
export const LINK =
  "shrink-0 rounded text-[12.5px] font-semibold hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";

/** A field that sits in a section: the masked token, a token or domain input. */
export const FIELD = "h-[34px] rounded-lg bg-background font-mono text-[12.5px]";

/** The row at the bottom of a pane: a destructive link on the left, Done on the right. */
export const FOOTER = "flex items-center justify-between gap-3 pt-0.5";
