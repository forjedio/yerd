<script setup lang="ts">
import { Check, ChevronDown, Copy, Inbox, MailX, Trash2 } from "lucide-vue-next";
import { computed, ref } from "vue";

import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import Spinner from "@/components/ui/Spinner.vue";
import type { MailStatus, MailSummary } from "@/ipc/types";
import { type AppOption, listTime, mailApp } from "@/lib/mail";
import { cn } from "@/lib/utils";

/**
 * The inbox: an "Inbox · n unread" header with the app filter menu and Clear
 * all, then the messages. `list` is the column beside the reader (wide windows
 * and the Mails window); `table` spans the window (compact) with SUBJECT, APP,
 * TO and RECEIVED columns, where a click opens the reader drawer and there is
 * no persistent selection. `capture` is the main window's mail status, which
 * picks the empty state; the Mails window omits it.
 */
const props = defineProps<{
  layout: "list" | "table";
  /** The mails of the active filter. */
  mails: MailSummary[];
  /** Every mail in the mailbox, whatever the filter. */
  total: number;
  unread: number;
  apps: AppOption[];
  selected: string | null;
  /** Epoch seconds, for the row times. */
  now: number;
  loading: boolean;
  error: string | null;
  clearing: boolean;
  capture?: MailStatus | null;
}>();

const app = defineModel<string>("app", { required: true });

const emit = defineEmits<{
  select: [id: string];
  clearAll: [];
  retry: [];
  copyEnv: [];
  turnOn: [];
}>();

const confirmOpen = ref(false);

const recipients = (m: MailSummary): string => m.to.join(", ");
const subject = (m: MailSummary): string => m.subject?.trim() || "(no subject)";
const unreadRow = (m: MailSummary): boolean => m.read === false;

const captureOff = computed(() => !!props.capture && !props.capture.enabled);
const emptyCopy = computed(() =>
  props.capture
    ? `Point your app's mailer at 127.0.0.1:${props.capture.port} and anything it sends lands here.`
    : "Point your app's mailer at Yerd's SMTP server and anything it sends lands here.",
);

const TABLE_COLS = "grid grid-cols-[minmax(0,1fr)_120px_200px_100px] items-center gap-x-3";
</script>

<template>
  <div class="flex min-h-0 flex-col" data-testid="mail-inbox" :data-layout="layout">
    <div class="flex h-[46px] shrink-0 items-center gap-2 pr-4">
      <h3 class="text-[15px] font-semibold">Inbox</h3>
      <span
        v-if="unread > 0"
        class="rounded-full bg-brand/[0.14] px-2 py-0.5 text-[11px] font-medium text-brand"
        data-testid="unread-chip"
      >
        {{ unread }} unread
      </span>
      <div class="ml-auto flex items-center gap-2">
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button
              variant="outline"
              size="sm"
              class="h-[30px] max-w-[180px] gap-1.5 px-2.5"
              :disabled="total === 0 && !app"
              aria-label="Filter by app"
              data-testid="app-filter"
            >
              <span class="truncate">{{ app || "All apps" }}</span>
              <ChevronDown class="text-muted-foreground" aria-hidden="true" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-[220px]">
            <DropdownMenuItem
              :class="cn(app === '' && 'bg-brand/[0.14]')"
              data-app=""
              @select="app = ''"
            >
              <span class="flex-1 truncate">All apps</span>
              <span class="font-mono text-[11px] text-faint">{{ total }}</span>
              <Check :class="cn('size-3.5 text-brand', app === '' ? 'opacity-100' : 'opacity-0')" aria-hidden="true" />
            </DropdownMenuItem>
            <DropdownMenuItem
              v-for="o in apps"
              :key="o.value"
              :class="cn(app === o.value && 'bg-brand/[0.14]')"
              :data-app="o.value"
              @select="app = o.value"
            >
              <span class="flex-1 truncate">{{ o.label }}</span>
              <span class="font-mono text-[11px] text-faint">{{ o.count }}</span>
              <Check :class="cn('size-3.5 text-brand', app === o.value ? 'opacity-100' : 'opacity-0')" aria-hidden="true" />
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <Button
          variant="outline"
          size="icon"
          class="size-[30px]"
          :disabled="total === 0 || clearing"
          aria-label="Clear all messages"
          title="Clear all messages"
          @click="confirmOpen = true"
        >
          <Spinner v-if="clearing" class="size-3.5" />
          <Trash2 v-else class="!size-3.5" />
        </Button>
      </div>
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto pb-3 pr-4">
      <div v-if="loading && total === 0" class="flex h-full items-center justify-center py-16">
        <Spinner class="size-5" />
      </div>

      <div
        v-else-if="error && total === 0"
        class="flex h-full flex-col items-center justify-center px-6 py-16 text-center"
        data-testid="inbox-error"
      >
        <p class="text-[13px] text-muted-foreground">{{ error }}</p>
        <Button variant="outline" size="sm" class="mt-4" @click="emit('retry')">Retry</Button>
      </div>

      <div
        v-else-if="total === 0"
        class="flex h-full flex-col items-center justify-center px-6 py-16 text-center"
        data-testid="inbox-empty"
      >
        <span class="flex size-11 items-center justify-center rounded-xl border bg-card text-muted-foreground">
          <MailX v-if="captureOff" class="size-5" aria-hidden="true" />
          <Inbox v-else class="size-5" aria-hidden="true" />
        </span>
        <template v-if="captureOff">
          <p class="mt-4 text-[15px] font-semibold">Mail capture is off</p>
          <p class="mt-1.5 max-w-sm text-[13px] text-muted-foreground">Turn it on to catch what your apps send.</p>
          <Button size="sm" class="mt-5" @click="emit('turnOn')">Turn on</Button>
        </template>
        <template v-else>
          <p class="mt-4 text-[15px] font-semibold">No mail yet</p>
          <p class="mt-1.5 max-w-sm text-[13px] text-muted-foreground">{{ emptyCopy }}</p>
          <Button v-if="capture" variant="outline" size="sm" class="mt-5" @click="emit('copyEnv')">
            <Copy />
            Copy Laravel .env
          </Button>
        </template>
      </div>

      <div
        v-else-if="mails.length === 0"
        class="flex h-full flex-col items-center justify-center px-6 py-16 text-center"
        data-testid="inbox-filtered-empty"
      >
        <p class="text-[15px] font-semibold">No mail from {{ app }}</p>
        <button
          type="button"
          class="mt-1.5 text-[13px] font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          @click="app = ''"
        >
          Show all apps
        </button>
      </div>

      <div v-else-if="layout === 'list'" class="flex flex-col gap-0.5">
        <button
          v-for="m in mails"
          :key="m.id"
          type="button"
          :data-id="m.id"
          :aria-current="selected === m.id ? 'true' : undefined"
          :class="
            cn(
              'flex h-[74px] w-full shrink-0 gap-2.5 rounded-lg border px-3 py-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
              selected === m.id ? 'border-brand/45 bg-brand/[0.14]' : 'border-transparent hover:bg-accent/60',
            )
          "
          @click="emit('select', m.id)"
        >
          <span class="flex w-2 shrink-0 justify-center pt-[5px]">
            <span v-if="unreadRow(m)" class="size-[7px] rounded-full bg-brand" aria-label="Unread" />
          </span>
          <span class="flex min-w-0 flex-1 flex-col">
            <span class="flex items-baseline gap-2">
              <span
                :class="
                  cn('min-w-0 flex-1 truncate text-xs font-medium', unreadRow(m) ? 'text-foreground' : 'text-muted-foreground')
                "
              >
                {{ mailApp(m) }}
              </span>
              <span class="shrink-0 text-[11px] text-faint">{{ listTime(m.date_epoch, now) }}</span>
            </span>
            <span
              :class="
                cn(
                  'mt-0.5 truncate text-[13.5px]',
                  unreadRow(m) ? 'font-semibold text-foreground' : 'text-muted-foreground',
                )
              "
            >
              {{ subject(m) }}
            </span>
            <span class="mt-0.5 truncate font-mono text-[11.5px] text-faint">{{ recipients(m) }}</span>
          </span>
        </button>
      </div>

      <template v-else>
        <div
          :class="cn(TABLE_COLS, 'px-2.5 pb-1.5 pt-1 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
          aria-hidden="true"
        >
          <span class="pl-4">Subject</span><span>App</span><span>To</span><span>Received</span>
        </div>
        <button
          v-for="m in mails"
          :key="m.id"
          type="button"
          :data-id="m.id"
          :class="
            cn(
              TABLE_COLS,
              'h-11 w-full rounded-lg px-2.5 text-left transition-colors hover:bg-accent/60 focus-visible:bg-brand/25 focus-visible:outline-none',
            )
          "
          @click="emit('select', m.id)"
        >
          <span class="flex min-w-0 items-center gap-2">
            <span class="flex w-2 shrink-0 justify-center">
              <span v-if="unreadRow(m)" class="size-[7px] rounded-full bg-brand" aria-label="Unread" />
            </span>
            <span :class="cn('truncate text-[13px]', unreadRow(m) ? 'font-semibold text-foreground' : 'text-muted-foreground')">
              {{ subject(m) }}
            </span>
          </span>
          <span class="min-w-0">
            <span
              class="inline-block max-w-full truncate rounded bg-muted px-1.5 py-0.5 align-middle text-[11px] text-muted-foreground"
              data-testid="app-chip"
            >
              {{ mailApp(m) }}
            </span>
          </span>
          <span class="truncate font-mono text-xs text-muted-foreground">{{ recipients(m) }}</span>
          <span class="truncate text-xs text-faint">{{ listTime(m.date_epoch, now) }}</span>
        </button>
      </template>
    </div>

    <ConfirmDialog
      v-model:open="confirmOpen"
      :title="total === 1 ? 'Clear 1 message?' : `Clear all ${total} messages?`"
      confirm-label="Clear all"
      tone="danger"
      :icon="Trash2"
      @confirm="emit('clearAll')"
    >
      Every captured email is deleted, whichever app filter is showing. This can't be undone.
    </ConfirmDialog>
  </div>
</template>
