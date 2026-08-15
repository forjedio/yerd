import { readonly, ref } from "vue";

export type ToastKind = "success" | "error" | "info";

export interface Toast {
  id: number;
  kind: ToastKind;
  title: string;
  detail?: string;
}

// Module-level singleton store so any component can raise a toast and the single
// <Toaster> mounted in App.vue renders them.
const toasts = ref<Toast[]>([]);
let nextId = 1;

/** Raise a toast that auto-dismisses: errors linger, success and info clear
 *  sooner. `ttlMs` overrides that default, for a confirmation that shouldn't
 *  outstay its welcome (an idempotent action the user may repeat). */
function push(kind: ToastKind, title: string, detail?: string, ttlMs?: number): number {
  const id = nextId++;
  toasts.value = [...toasts.value, { id, kind, title, detail }];
  const ttl = ttlMs ?? (kind === "error" ? 8000 : 4000);
  setTimeout(() => dismiss(id), ttl);
  return id;
}

function dismiss(id: number): void {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

export function useToast() {
  return {
    toasts: readonly(toasts),
    success: (title: string, detail?: string, ttlMs?: number) =>
      push("success", title, detail, ttlMs),
    error: (title: string, detail?: string, ttlMs?: number) => push("error", title, detail, ttlMs),
    info: (title: string, detail?: string, ttlMs?: number) => push("info", title, detail, ttlMs),
    dismiss,
  };
}
