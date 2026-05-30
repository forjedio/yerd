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

function push(kind: ToastKind, title: string, detail?: string): number {
  const id = nextId++;
  toasts.value = [...toasts.value, { id, kind, title, detail }];
  // Errors linger; success/info auto-dismiss.
  const ttl = kind === "error" ? 8000 : 4000;
  setTimeout(() => dismiss(id), ttl);
  return id;
}

function dismiss(id: number): void {
  toasts.value = toasts.value.filter((t) => t.id !== id);
}

export function useToast() {
  return {
    toasts: readonly(toasts),
    success: (title: string, detail?: string) => push("success", title, detail),
    error: (title: string, detail?: string) => push("error", title, detail),
    info: (title: string, detail?: string) => push("info", title, detail),
    dismiss,
  };
}
