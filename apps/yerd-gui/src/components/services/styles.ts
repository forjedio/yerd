/** Shared classes for the Services screen's compact form fields, card shells and status dots. */
import type { ServiceRunState } from "@/ipc/types";

/** A 32px native text field, as the draft draws the inline add rows and filters. */
export const FIELD =
  "h-8 rounded-md border border-input bg-background px-2.5 font-mono text-xs outline-none transition-colors placeholder:font-sans placeholder:text-faint focus-visible:border-brand/60 focus-visible:ring-1 focus-visible:ring-brand/40 disabled:cursor-not-allowed disabled:opacity-50";

/** The inspector's card shell. */
export const CARD = "rounded-[10px] border bg-card";

/** The status dot's colour for a service state. */
export function stateDotClass(state: ServiceRunState): string {
  if (state === "running") return "bg-success";
  if (state === "failed") return "bg-destructive";
  return "bg-faint";
}
