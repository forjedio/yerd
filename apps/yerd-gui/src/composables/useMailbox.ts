import { computed, ref, watch, type ComputedRef, type Ref } from "vue";

import { usePoll } from "@/composables/usePoll";
import { useToast } from "@/composables/useToast";
import {
  clearMails,
  deleteMails,
  getMail,
  IpcError,
  listMails,
  markMailsRead,
} from "@/ipc/client";
import type { MailDetail, MailSummary } from "@/ipc/types";
import { type AppOption, appOptions, filterByApp, unreadCount } from "@/lib/mail";
import { log } from "@/lib/log";

/** Options for {@link useMailbox}. */
export interface MailboxOptions {
  /** Whether to select the first message on its own. A getter, so a window
   *  resize can turn it on and off. The automatic pick is never marked read. */
  autoSelect: () => boolean;
}

/** The mailbox state and actions shared by the Mail screen and the Mails window. */
export interface Mailbox {
  /** Every captured mail, newest first. */
  list: ComputedRef<MailSummary[]>;
  /** The mails of the active app filter. */
  filtered: ComputedRef<MailSummary[]>;
  /** The active app filter; `""` shows every app. */
  app: Ref<string>;
  apps: ComputedRef<AppOption[]>;
  unread: ComputedRef<number>;
  /** Epoch seconds, refreshed on each poll so day labels roll over. */
  now: Ref<number>;
  /** The first list has not arrived yet. */
  loading: ComputedRef<boolean>;
  /** The list could not be fetched and none is cached. */
  error: ComputedRef<string | null>;
  selectedId: Ref<string | null>;
  detail: Ref<MailDetail | null>;
  loadingDetail: Ref<boolean>;
  busy: Ref<"delete" | "clear" | null>;
  select: (id: string, fromUser: boolean) => Promise<void>;
  clearSelection: () => void;
  deleteMail: (id: string) => Promise<boolean>;
  clearAll: () => Promise<boolean>;
  refresh: () => Promise<void>;
}

function nowSeconds(): number {
  return Math.floor(Date.now() / 1000);
}

/**
 * Poll the captured mail every 4s and track the app filter, the selected
 * message and its detail. A user's selection marks the message read; the
 * automatic one does not. A sequence number drops a stale `get_mail` reply when
 * the selection moves on before it arrives. A user's pick that fails to load is
 * dropped, so no row stays highlighted over an empty reader; a failed automatic
 * pick is kept so the next poll doesn't retry it and toast again.
 */
export function useMailbox(options: MailboxOptions): Mailbox {
  const toast = useToast();
  const { data: mails, error: pollError, refresh } = usePoll<MailSummary[]>(listMails, 4000);

  const list = computed<MailSummary[]>(() => mails.value ?? []);
  const app = ref("");
  const apps = computed(() => appOptions(list.value, app.value));
  const filtered = computed(() => filterByApp(list.value, app.value));
  const unread = computed(() => unreadCount(list.value));
  const now = ref(nowSeconds());
  const loading = computed(() => mails.value === null && !pollError.value);
  const error = computed(() => (mails.value === null ? (pollError.value?.message ?? null) : null));

  const selectedId = ref<string | null>(null);
  const detail = ref<MailDetail | null>(null);
  const loadingDetail = ref(false);
  const busy = ref<"delete" | "clear" | null>(null);
  let selectSeq = 0;

  watch(mails, () => {
    now.value = nowSeconds();
  });

  function clearSelection(): void {
    selectSeq += 1;
    selectedId.value = null;
    detail.value = null;
    loadingDetail.value = false;
  }

  async function markRead(id: string): Promise<void> {
    const current = mails.value?.find((m) => m.id === id);
    if (!current || current.read !== false) return;
    try {
      await markMailsRead([id]);
      if (mails.value) {
        mails.value = mails.value.map((m) => (m.id === id ? { ...m, read: true } : m));
      }
    } catch (e) {
      log.warn(`mark mail read failed: ${(e as IpcError).message}`);
    }
  }

  async function select(id: string, fromUser: boolean): Promise<void> {
    const seq = ++selectSeq;
    selectedId.value = id;
    loadingDetail.value = true;
    try {
      const mail = await getMail(id);
      if (seq !== selectSeq) return;
      detail.value = mail;
      if (fromUser) void markRead(id);
    } catch (e) {
      if (seq !== selectSeq) return;
      toast.error("Couldn't open the email", (e as IpcError).message);
      if (fromUser) clearSelection();
      else detail.value = null;
    } finally {
      if (seq === selectSeq) loadingDetail.value = false;
    }
  }

  function syncSelection(): void {
    const items = filtered.value;
    if (selectedId.value && !items.some((m) => m.id === selectedId.value)) clearSelection();
    const first = items[0];
    if (options.autoSelect() && !selectedId.value && first) void select(first.id, false);
  }

  watch(filtered, syncSelection, { immediate: true });
  watch(options.autoSelect, (on) => {
    if (on) syncSelection();
  });

  async function deleteMail(id: string): Promise<boolean> {
    if (busy.value) return false;
    busy.value = "delete";
    try {
      await deleteMails([id]);
      if (selectedId.value === id) clearSelection();
      await refresh();
      toast.success("Message deleted");
      return true;
    } catch (e) {
      toast.error("Couldn't delete the message", (e as IpcError).message);
      return false;
    } finally {
      busy.value = null;
    }
  }

  async function clearAll(): Promise<boolean> {
    if (busy.value) return false;
    busy.value = "clear";
    try {
      await clearMails();
      clearSelection();
      app.value = "";
      await refresh();
      toast.success("Mailbox cleared");
      return true;
    } catch (e) {
      toast.error("Couldn't clear the mailbox", (e as IpcError).message);
      return false;
    } finally {
      busy.value = null;
    }
  }

  return {
    list,
    filtered,
    app,
    apps,
    unread,
    now,
    loading,
    error,
    selectedId,
    detail,
    loadingDetail,
    busy,
    select,
    clearSelection,
    deleteMail,
    clearAll,
    refresh,
  };
}
