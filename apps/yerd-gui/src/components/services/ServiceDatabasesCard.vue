<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";
import { Download, Ellipsis, Plug, Plus, Search, Trash2, Upload } from "lucide-vue-next";

import { FIELD } from "@/components/services/styles";
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
import Switch from "@/components/ui/Switch.vue";
import { useToast } from "@/composables/useToast";
import {
  backupDatabase,
  createDatabase,
  dropDatabase,
  IpcError,
  pickOpenFile,
  pickSaveFile,
  restoreDatabase,
} from "@/ipc/client";
import type { ServiceStatus } from "@/ipc/types";
import { databaseExportFilename } from "@/lib/databaseFilename";
import {
  databasesFooter,
  DB_NAME_HINT,
  fileName,
  groupDatabases,
  isValidDbName,
} from "@/lib/services";
import { cn } from "@/lib/utils";

/**
 * The inspector's Databases card for a SQL engine: create, filter, fold
 * Laravel's parallel-testing copies under the database they came from, and a
 * row menu to back up, restore, use in Connect or delete. Delete and restore
 * confirm in place, in the row itself. The view owns the list (so the list
 * row's count, the Connect picker and this card agree); this card runs the
 * mutations and emits `changed` for the view to re-list.
 *
 * `layerOpen` is true while a row menu is open, so the drawer leaves Escape to
 * it; the inline inputs and confirm rows stop Escape themselves.
 */
const props = defineProps<{
  service: ServiceStatus;
  names: string[] | null;
  loading: boolean;
  error: string | null;
  /** The database the Connect card shows, highlighted in the list. */
  connectDb: string;
}>();

const emit = defineEmits<{
  changed: [];
  useInConnect: [name: string];
}>();

const layerOpen = defineModel<boolean>("layerOpen", { default: false });

const toast = useToast();

const filter = ref("");
const hideCopies = ref(true);
const creating = ref(false);
const newName = ref("");
const confirm = ref<{ kind: "drop" | "restore"; name: string; path?: string } | null>(null);
const busy = ref(false);
const menuOpen = ref<Record<string, boolean>>({});
const cancelButton = ref<HTMLButtonElement | null>(null);
const createInput = ref<HTMLInputElement | null>(null);

const running = computed(() => props.service.state === "running");
const all = computed(() => props.names ?? []);
const grouped = computed(() =>
  groupDatabases(all.value, { hideCopies: hideCopies.value, filter: filter.value }),
);
const footer = computed(() => databasesFooter(grouped.value.counts, hideCopies.value));
const trimmedName = computed(() => newName.value.trim());
const nameInvalid = computed(() => trimmedName.value !== "" && !isValidDbName(trimmedName.value));

watch(
  menuOpen,
  (open) => {
    layerOpen.value = Object.values(open).some(Boolean);
  },
  { deep: true },
);

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

async function doCreateDb(): Promise<void> {
  const name = trimmedName.value;
  if (!isValidDbName(name) || busy.value) return;
  busy.value = true;
  try {
    await createDatabase(props.service.service, name);
    toast.success(`Created database ${name}`);
    cancelCreate();
    emit("changed");
  } catch (e) {
    toast.error("Couldn't create database", (e as IpcError).message);
  } finally {
    busy.value = false;
  }
}

function later(fn: () => void): void {
  void nextTick(fn);
}

function askDrop(name: string): void {
  later(() => {
    confirm.value = { kind: "drop", name };
  });
}

async function doDropDb(name: string): Promise<void> {
  busy.value = true;
  confirm.value = null;
  try {
    await dropDatabase(props.service.service, name);
    toast.success(`Dropped database ${name}`);
    emit("changed");
  } catch (e) {
    toast.error("Couldn't drop database", (e as IpcError).message);
  } finally {
    busy.value = false;
  }
}

async function doBackupDb(name: string): Promise<void> {
  const path = await pickSaveFile(databaseExportFilename(name));
  if (!path) return;
  busy.value = true;
  try {
    await backupDatabase(props.service.service, name, path);
    toast.success(`Backed up ${name}`, `Saved to ${path}`);
  } catch (e) {
    toast.error("Couldn't back up database", (e as IpcError).message);
  } finally {
    busy.value = false;
  }
}

/** Pick the file first, then confirm in the row (restore overwrites data). */
async function startRestoreDb(name: string): Promise<void> {
  const path = await pickOpenFile();
  if (!path) return;
  confirm.value = { kind: "restore", name, path };
}

async function doRestoreDb(): Promise<void> {
  const pending = confirm.value;
  if (!pending?.path) return;
  busy.value = true;
  confirm.value = null;
  try {
    await restoreDatabase(props.service.service, pending.name, pending.path);
    toast.success(`Restored ${pending.name}`);
    emit("changed");
  } catch (e) {
    toast.error("Couldn't restore database", (e as IpcError).message);
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
  confirm.value = null;
}

const ROW = "flex h-9 items-center gap-2 pl-4 pr-2";
const CONFIRM_BTN =
  "inline-flex h-7 shrink-0 items-center rounded-md px-3 text-xs font-semibold transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";
</script>

<template>
  <section :class="cn(CARD, 'flex min-w-0 flex-col')" data-testid="databases-card">
    <header class="flex h-[54px] shrink-0 items-center gap-2 px-4">
      <h3 class="text-sm font-semibold">Databases</h3>
      <span
        v-if="names"
        class="inline-flex h-[18px] items-center rounded bg-muted px-1.5 font-mono text-[11px] text-muted-foreground"
        data-testid="databases-count"
      >
        {{ all.length }}
      </span>
      <button
        type="button"
        class="ml-auto inline-flex h-[30px] items-center gap-1.5 rounded-md border border-brand/30 bg-brand/15 px-3 text-[13px] font-medium text-brand transition-colors hover:bg-brand/25 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
        :disabled="!running || busy"
        @click="openCreate"
      >
        <Plus class="size-3.5" aria-hidden="true" /> New database
      </button>
    </header>

    <div v-if="creating" class="px-4 pb-3" @keydown.esc.stop="cancelCreate">
      <div class="flex gap-2">
        <input
          ref="createInput"
          v-model="newName"
          :class="cn(FIELD, 'min-w-0 flex-1')"
          placeholder="my_app"
          aria-label="New database name"
          spellcheck="false"
          autocomplete="off"
          :disabled="busy"
          @keydown.enter="doCreateDb"
        />
        <Button variant="ghost" size="sm" :disabled="busy" @click="cancelCreate">Cancel</Button>
        <Button size="sm" :disabled="!isValidDbName(trimmedName) || busy" @click="doCreateDb">
          <Spinner v-if="busy" class="size-4" />
          Create
        </Button>
      </div>
      <p v-if="nameInvalid" class="mt-1.5 text-xs text-destructive">{{ DB_NAME_HINT }}</p>
    </div>

    <template v-if="!running">
      <p class="border-t px-4 py-6 text-center text-[13px] text-muted-foreground">
        Start {{ service.display_name }} to see its databases.
      </p>
    </template>

    <template v-else>
      <div class="flex shrink-0 items-center gap-2 px-2.5 pb-2.5">
        <div class="relative min-w-0 flex-1">
          <Search
            class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-faint"
            aria-hidden="true"
          />
          <input
            v-model="filter"
            :class="cn(FIELD, 'w-full pl-8 font-sans')"
            placeholder="Filter databases"
            aria-label="Filter databases"
            spellcheck="false"
            autocomplete="off"
            @keydown.esc="clearFilter"
          />
        </div>
        <label class="flex h-8 shrink-0 cursor-pointer items-center gap-2 rounded-md border bg-background px-2.5 text-xs text-muted-foreground">
          <Switch v-model="hideCopies" aria-label="Hide test copies" class="scale-90" />
          Hide test copies
        </label>
      </div>

      <div v-if="loading && !names" class="flex justify-center border-t py-8">
        <Spinner class="size-5" />
      </div>
      <p v-else-if="error" class="border-t px-4 py-6 text-center text-[13px] text-muted-foreground">
        {{ error }}
      </p>
      <p v-else-if="!all.length" class="border-t px-4 py-6 text-center text-[13px] text-muted-foreground">
        No databases yet.
      </p>
      <p
        v-else-if="!grouped.rows.length"
        class="border-t px-4 py-6 text-center text-[13px] text-muted-foreground"
      >
        No databases match "{{ filter.trim() }}".
      </p>
      <ul v-else class="max-h-[470px] overflow-y-auto border-t py-1" data-testid="databases-list">
        <li v-for="row in grouped.rows" :key="row.name" :data-db="row.name">
          <div
            v-if="confirm?.name === row.name && confirm.kind === 'drop'"
            class="mx-1.5 flex min-h-12 items-center gap-2 rounded-md border border-destructive/40 bg-destructive/10 px-3 py-1.5"
            data-testid="confirm-drop"
            @keydown.esc.stop="cancelConfirm"
          >
            <span class="min-w-0 flex-1 text-xs text-destructive">
              Delete <span class="font-mono">{{ row.name }}</span>? This can't be undone.
            </span>
            <button
              :ref="setCancelButton"
              type="button"
              :class="cn(CONFIRM_BTN, 'text-muted-foreground hover:bg-accent hover:text-foreground')"
              @click="cancelConfirm"
            >
              Cancel
            </button>
            <button
              type="button"
              :class="cn(CONFIRM_BTN, 'bg-destructive text-destructive-foreground hover:bg-destructive/90')"
              :disabled="busy"
              @click="doDropDb(row.name)"
            >
              Delete
            </button>
          </div>
          <div
            v-else-if="confirm?.name === row.name && confirm.kind === 'restore'"
            class="mx-1.5 flex min-h-12 items-center gap-2 rounded-md border border-warning/40 bg-warning/15 px-3 py-1.5"
            data-testid="confirm-restore"
            @keydown.esc.stop="cancelConfirm"
          >
            <span class="min-w-0 flex-1 truncate text-xs text-warning" :title="confirm.path">
              Overwrite <span class="font-mono">{{ row.name }}</span> with
              <span class="font-mono">{{ fileName(confirm.path ?? "") }}</span>?
            </span>
            <button
              :ref="setCancelButton"
              type="button"
              :class="cn(CONFIRM_BTN, 'text-muted-foreground hover:bg-accent hover:text-foreground')"
              @click="cancelConfirm"
            >
              Cancel
            </button>
            <button
              type="button"
              :class="cn(CONFIRM_BTN, 'bg-warning text-background hover:bg-warning/90')"
              :disabled="busy"
              @click="doRestoreDb"
            >
              Restore
            </button>
          </div>
          <div
            v-else
            :class="cn(ROW, 'mx-1.5 rounded-md', row.name === connectDb && 'bg-brand/[0.08]')"
          >
            <span class="min-w-0 flex-1 truncate font-mono text-[13px]">{{ row.name }}</span>
            <span
              v-if="row.copies"
              :class="cn('shrink-0 text-[11px]', row.name === connectDb ? 'text-brand' : 'text-faint')"
              :title="`${row.copies} parallel test cop${row.copies === 1 ? 'y' : 'ies'} hidden`"
            >
              + {{ row.copies }} test
            </span>
            <DropdownMenu v-model:open="menuOpen[row.name]">
              <DropdownMenuTrigger as-child>
                <button
                  type="button"
                  class="flex size-6 shrink-0 items-center justify-center rounded text-faint transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
                  :aria-label="`Actions for ${row.name}`"
                >
                  <Ellipsis class="size-4" />
                </button>
              </DropdownMenuTrigger>
              <DropdownMenuContent align="end" class="w-[220px]">
                <DropdownMenuItem :disabled="busy" data-action="backup" @select="doBackupDb(row.name)">
                  <Download class="size-4" /> Back up to file…
                </DropdownMenuItem>
                <DropdownMenuItem :disabled="busy" data-action="restore" @select="startRestoreDb(row.name)">
                  <Upload class="size-4" /> Restore from file…
                </DropdownMenuItem>
                <DropdownMenuItem
                  :disabled="busy"
                  data-action="use"
                  @select="emit('useInConnect', row.name)"
                >
                  <Plug class="size-4" /> Use in Connect
                </DropdownMenuItem>
                <DropdownMenuSeparator />
                <DropdownMenuItem
                  :disabled="busy"
                  class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                  data-action="drop"
                  @select="askDrop(row.name)"
                >
                  <Trash2 class="size-4" /> Delete database
                </DropdownMenuItem>
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </li>
      </ul>

      <footer
        v-if="all.length"
        class="mt-auto flex h-[42px] shrink-0 items-center border-t px-4 text-xs text-faint"
        data-testid="databases-footer"
      >
        {{ footer }}
      </footer>
    </template>
  </section>
</template>
