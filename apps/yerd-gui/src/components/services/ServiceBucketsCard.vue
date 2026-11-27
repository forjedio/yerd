<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { Ellipsis, HardDrive, Package, Play, Plug, Plus, Search, Trash2 } from "lucide-vue-next";

import { CONFIRM_BTN, FIELD, ROW } from "@/components/services/styles";
import { CARD } from "@/components/ui/styles";
import Button from "@/components/ui/Button.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import type { S3BucketStat } from "@/composables/useS3Storage";
import { createBucket, dropBucket, IpcError } from "@/ipc/client";
import type { BucketStats, ServiceStatus } from "@/ipc/types";
import {
  BUCKET_NAME_HINT,
  bucketNameError,
  bucketsFooter,
  formatBytes,
  objectsLabel,
} from "@/lib/services";
import { cn } from "@/lib/utils";

/**
 * The inspector's Buckets card for S3 storage: filter, rows with each
 * bucket's object count and size, an inline create row with the S3 naming
 * rules, and a row menu to use a bucket in Connect or delete it (confirmed in
 * place, in the row). The view owns the list and stats (so the list row's
 * count, the Connect picker and this card agree); this card runs the
 * mutations, emits `changed` after a create and `dropped` after a delete.
 * Buckets live in the running gateway, so a stopped service shows a Start
 * prompt instead.
 *
 * `layerOpen` is true while a row menu is open, so the drawer leaves Escape to
 * it; the inline input and confirm row stop Escape themselves.
 */
const props = defineProps<{
  service: ServiceStatus;
  names: string[] | null;
  loading: boolean;
  error: string | null;
  stats: Record<string, S3BucketStat>;
  /** The gateway endpoint, for the created toast's bucket URL. */
  endpoint: string;
  /** The bucket the Connect card shows, highlighted in the list. */
  connectBucket: string;
}>();

const emit = defineEmits<{
  changed: [];
  dropped: [name: string];
  useInConnect: [name: string];
  start: [];
}>();

const layerOpen = defineModel<boolean>("layerOpen", { default: false });

const toast = useToast();

const filter = ref("");
const creating = ref(false);
const newName = ref("");
const confirm = ref<string | null>(null);
const busy = ref(false);
const menuOpen = ref<Record<string, boolean>>({});
const cancelButton = ref<HTMLButtonElement | null>(null);
const createInput = ref<HTMLInputElement | null>(null);

const COUNT = new Intl.NumberFormat("en-US");

const running = computed(() => props.service.state === "running");
const all = computed(() => props.names ?? []);
const empty = computed(() => props.names !== null && all.value.length === 0);
const rows = computed(() => {
  const q = filter.value.trim().toLowerCase();
  return q ? all.value.filter((n) => n.toLowerCase().includes(q)) : all.value;
});
const trimmedName = computed(() => newName.value.trim());
const nameError = computed(() => (trimmedName.value === "" ? null : bucketNameError(trimmedName.value)));
const canCreate = computed(() => trimmedName.value !== "" && nameError.value === null && !busy.value);

function statOf(name: string): BucketStats | null {
  const s = props.stats[name];
  return s && typeof s === "object" ? s : null;
}

function statPending(name: string): boolean {
  const s = props.stats[name];
  return s === undefined || s === "loading";
}

function objectsCell(name: string): string {
  if (statPending(name)) return "…";
  const s = statOf(name);
  return s ? objectsLabel(s.objects) : "—";
}

function sizeCell(name: string): string {
  if (statPending(name)) return "…";
  const s = statOf(name);
  return s && s.objects > 0 ? formatBytes(s.bytes) : "—";
}

const footer = computed(() => {
  if (!all.value.length) return null;
  let objects = 0;
  let bytes = 0;
  for (const name of all.value) {
    const s = statOf(name);
    if (!s) return null;
    objects += s.objects;
    bytes += s.bytes;
  }
  return bucketsFooter(all.value.length, objects, bytes);
});

watch(
  menuOpen,
  (open) => {
    layerOpen.value = Object.values(open).some(Boolean);
  },
  { deep: true },
);

/** A row menu can unmount while open (the service stops, the row is filtered
 *  out, a confirm replaces it) without emitting `update:open(false)`, so its
 *  flag is dropped here or the drawer would keep leaving Escape to it. */
function forgetMenu(name: string): void {
  if (!(name in menuOpen.value)) return;
  const { [name]: _gone, ...rest } = menuOpen.value;
  menuOpen.value = rest;
  layerOpen.value = Object.values(rest).some(Boolean);
}

onBeforeUnmount(() => {
  layerOpen.value = false;
});

watch(
  confirm,
  async (c) => {
    if (!c) return;
    await nextTick();
    cancelButton.value?.focus();
  },
  { flush: "post" },
);

function setCancelButton(el: unknown): void {
  cancelButton.value = el instanceof HTMLButtonElement ? el : null;
}

async function openCreate(): Promise<void> {
  creating.value = true;
  newName.value = "";
  await nextTick();
  createInput.value?.focus();
}

function cancelCreate(): void {
  creating.value = false;
  newName.value = "";
}

async function doCreate(): Promise<void> {
  const name = trimmedName.value;
  if (!canCreate.value) return;
  busy.value = true;
  try {
    await createBucket(name);
    toast.success(`Created bucket ${name}`, `Ready at ${props.endpoint}/${name}`);
    cancelCreate();
    emit("changed");
  } catch (e) {
    const err = e as IpcError;
    const detail = err.code === "already_exists" ? `A bucket called ${name} already exists` : err.message;
    toast.error("Couldn't create bucket", detail);
  } finally {
    busy.value = false;
  }
}

function askDrop(name: string): void {
  void nextTick(() => {
    confirm.value = name;
  });
}

function confirmText(name: string): string {
  const s = statOf(name);
  if (!s || s.objects === 0) return "? This can't be undone.";
  return ` and its ${objectsLabel(s.objects)}? This can't be undone.`;
}

async function doDrop(name: string): Promise<void> {
  busy.value = true;
  try {
    const removed = await dropBucket(name);
    confirm.value = null;
    toast.success(`Deleted bucket ${name}`, `${COUNT.format(removed)} ${removed === 1 ? "object" : "objects"} removed`);
    emit("dropped", name);
  } catch (e) {
    confirm.value = null;
    toast.error("Couldn't delete bucket", (e as IpcError).message);
  } finally {
    busy.value = false;
  }
}

/** Escape clears a non-empty filter and stops there; an empty one lets Escape
 *  through to the drawer. */
function clearFilter(event: KeyboardEvent): void {
  if (!filter.value) return;
  event.stopPropagation();
  filter.value = "";
}

function cancelConfirm(): void {
  if (!busy.value) confirm.value = null;
}

const NEW_BUCKET =
  "ml-auto inline-flex h-[30px] items-center gap-1.5 rounded-md border border-brand/30 bg-brand/15 px-3 text-[13px] font-medium text-brand transition-colors hover:bg-brand/25 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";
const STATE_ICON = "flex size-10 items-center justify-center rounded-lg border bg-background text-muted-foreground";
const COLS = "grid grid-cols-[minmax(0,1fr)_auto_64px_24px] items-center gap-2.5";
</script>

<template>
  <section :class="cn(CARD, 'flex min-h-[420px] min-w-0 flex-col')" data-testid="buckets-card">
    <header class="flex h-[54px] shrink-0 items-center gap-2 px-4">
      <h3 class="text-sm font-semibold">Buckets</h3>
      <span
        v-if="running && names && names.length"
        class="inline-flex h-[18px] items-center rounded bg-muted px-1.5 font-mono text-[11px] text-muted-foreground"
        data-testid="buckets-count"
      >
        {{ all.length }}
      </span>
      <button
        v-if="running && !empty"
        type="button"
        :class="NEW_BUCKET"
        :disabled="busy"
        data-testid="new-bucket"
        @click="openCreate"
      >
        <Plus class="size-3.5" aria-hidden="true" /> New bucket
      </button>
    </header>

    <div
      v-if="!running"
      class="flex flex-1 flex-col items-center justify-center border-t px-6 py-12 text-center"
      data-testid="buckets-stopped"
    >
      <span :class="STATE_ICON"><HardDrive class="size-[18px]" aria-hidden="true" /></span>
      <p class="mt-4 text-[15px] font-semibold">S3 is stopped</p>
      <p class="mt-1.5 max-w-[340px] text-[13px] leading-relaxed text-muted-foreground">
        Start {{ service.display_name }} to list and manage buckets. Your buckets are safe on disk.
      </p>
      <Button variant="outline" size="sm" class="mt-4 h-8 gap-1.5 px-3.5 text-[13px]" @click="emit('start')">
        <Play /> Start S3
      </Button>
    </div>

    <template v-else>
      <div v-if="creating" class="border-b px-3.5 pb-3" @keydown.esc.stop="cancelCreate">
        <div class="flex gap-2">
          <input
            ref="createInput"
            v-model="newName"
            :class="
              cn(
                FIELD,
                'h-[30px] min-w-0 flex-1 text-[13px]',
                nameError ? 'border-destructive focus-visible:border-destructive focus-visible:ring-destructive/40' : 'border-brand/60',
              )
            "
            placeholder="user-uploads"
            aria-label="New bucket name"
            :aria-invalid="nameError !== null"
            spellcheck="false"
            autocomplete="off"
            :disabled="busy"
            @keydown.enter="doCreate"
          />
          <Button size="sm" class="h-[30px] px-4 text-[13px] font-semibold" :disabled="!canCreate" @click="doCreate">
            <Spinner v-if="busy" class="size-4" />
            Create
          </Button>
          <Button variant="outline" size="sm" class="h-[30px] px-3.5 text-[13px]" :disabled="busy" @click="cancelCreate">
            Cancel
          </Button>
        </div>
        <p v-if="nameError" class="mt-2 text-xs text-destructive" data-testid="bucket-name-error">{{ nameError }}</p>
        <p v-else class="mt-2 text-xs leading-relaxed text-faint">{{ BUCKET_NAME_HINT }}</p>
      </div>

      <div v-if="loading && !names" class="flex flex-1 justify-center border-t py-8">
        <Spinner class="size-5" />
      </div>
      <p v-else-if="error" class="border-t px-4 py-6 text-center text-[13px] text-muted-foreground" data-testid="buckets-error">
        {{ error }}
      </p>
      <div
        v-else-if="empty && !creating"
        class="flex flex-1 flex-col items-center justify-center border-t px-6 py-12 text-center"
        data-testid="buckets-empty"
      >
        <span :class="STATE_ICON"><Package class="size-[18px]" aria-hidden="true" /></span>
        <p class="mt-4 text-[15px] font-semibold">No buckets yet</p>
        <p class="mt-1.5 max-w-[320px] text-[13px] leading-relaxed text-muted-foreground">
          Create a bucket, then point your app's AWS_BUCKET at it from the Connect card.
        </p>
        <Button size="sm" class="mt-4 h-8 gap-1.5 px-3.5 text-[13px] font-semibold" @click="openCreate">
          <Plus /> New bucket
        </Button>
      </div>

      <template v-else-if="names">
        <div v-if="!creating" class="shrink-0 px-2.5 pb-2.5">
          <div class="relative">
            <Search
              class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint"
              aria-hidden="true"
            />
            <input
              v-model="filter"
              :class="cn(FIELD, 'w-full pl-8 font-sans')"
              placeholder="Filter buckets"
              aria-label="Filter buckets"
              spellcheck="false"
              autocomplete="off"
              @keydown.esc="clearFilter"
            />
          </div>
        </div>

        <div
          v-if="!creating"
          :class="cn(COLS, 'h-8 shrink-0 border-y pl-4 pr-3.5 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
        >
          <span>Name</span>
          <span class="text-right">Objects</span>
          <span class="text-right">Size</span>
          <span />
        </div>

        <p v-if="!rows.length && all.length" class="px-4 py-6 text-center text-[13px] text-muted-foreground">
          No buckets match "{{ filter.trim() }}".
        </p>
        <ul v-else-if="rows.length" class="max-h-[470px] overflow-y-auto py-1" data-testid="buckets-list">
          <li v-for="name in rows" :key="name" :data-bucket="name">
            <div
              v-if="confirm === name"
              class="mx-1.5 flex min-h-12 items-center gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-1.5"
              data-testid="confirm-drop"
              @keydown.esc.stop="cancelConfirm"
            >
              <span class="min-w-0 flex-1 text-xs text-destructive">
                Delete <span class="font-mono">{{ name }}</span>{{ confirmText(name) }}
              </span>
              <button
                :ref="setCancelButton"
                type="button"
                :class="cn(CONFIRM_BTN, 'text-muted-foreground hover:bg-accent hover:text-foreground')"
                :disabled="busy"
                @click="cancelConfirm"
              >
                Cancel
              </button>
              <button
                type="button"
                :class="cn(CONFIRM_BTN, 'gap-1.5 bg-destructive text-destructive-foreground hover:bg-destructive/90')"
                :disabled="busy"
                @click="doDrop(name)"
              >
                <Spinner v-if="busy" class="size-3.5" />
                Delete
              </button>
            </div>
            <div
              v-else
              :class="cn(ROW, COLS, 'mx-1.5 rounded-md pl-2.5', name === connectBucket && 'bg-brand/[0.08]')"
            >
              <span class="min-w-0 truncate font-mono text-[13px]">{{ name }}</span>
              <span
                :class="cn('text-right text-xs', statOf(name)?.objects === 0 ? 'text-faint' : 'text-muted-foreground')"
                data-testid="bucket-objects"
              >
                {{ objectsCell(name) }}
              </span>
              <span class="text-right font-mono text-xs text-muted-foreground" data-testid="bucket-size">
                {{ sizeCell(name) }}
              </span>
              <DropdownMenu v-model:open="menuOpen[name]" @vue:before-unmount="forgetMenu(name)">
                <DropdownMenuTrigger as-child>
                  <button
                    type="button"
                    class="flex size-6 shrink-0 items-center justify-center rounded text-faint transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    :aria-label="`Actions for ${name}`"
                  >
                    <Ellipsis class="size-4" />
                  </button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="end" class="w-[220px]">
                  <DropdownMenuItem :disabled="busy" data-action="use" @select="emit('useInConnect', name)">
                    <Plug class="size-4" /> Use in Connect
                  </DropdownMenuItem>
                  <DropdownMenuSeparator />
                  <DropdownMenuItem
                    :disabled="busy"
                    class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                    data-action="drop"
                    @select="askDrop(name)"
                  >
                    <Trash2 class="size-4" /> Delete bucket
                  </DropdownMenuItem>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
          </li>
        </ul>

        <footer
          v-if="footer"
          class="mt-auto flex h-[42px] shrink-0 items-center border-t px-4 text-xs text-faint"
          data-testid="buckets-footer"
        >
          {{ footer }}
        </footer>
      </template>
    </template>
  </section>
</template>
