function str(v: unknown): string {
  return v === undefined || v === null ? "" : String(v);
}

function isEmptyContext(ctx: unknown): boolean {
  if (ctx === undefined || ctx === null || ctx === "") return true;
  return typeof ctx === "object" && Object.keys(ctx).length === 0;
}

/**
 * Body text for a Dumps window log row: `level message`, followed by the
 * pretty-printed `context` when it carries anything. PHP encodes an empty
 * context array as `[]`, so empty arrays count as no context, same as `{}`.
 */
export function logRowBody(payload: Record<string, unknown>): string {
  const head = `${str(payload.level)} ${str(payload.message)}`.trim();
  const ctx = payload.context;
  if (isEmptyContext(ctx)) return head;
  const json = JSON.stringify(ctx, null, 2);
  return head ? `${head}\n${json}` : json;
}
