/** Class strings shared by the redesigned screens' lists, inspectors and dialogs. */

/** An inspector card shell: the 10px-radius bordered card surface. */
export const CARD = "rounded-[10px] border bg-card";

/** A 34px inspector toolbar button (Open, Reveal, Restart, ...), passed to `Button`'s class. */
export const INSPECTOR_TOOL = "h-[34px] gap-1.5 px-3 text-[13px] font-semibold";

/** A small uppercase section label inside a dialog or panel. */
export const SECTION_LABEL = "text-[11px] font-semibold uppercase tracking-[0.6px] text-faint";

/** A list's group heading (e.g. Running / Stopped) above its rows. */
export const LIST_LABEL = `px-2 pb-1.5 pt-4 ${SECTION_LABEL}`;
