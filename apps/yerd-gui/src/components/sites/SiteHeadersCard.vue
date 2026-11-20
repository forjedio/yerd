<script setup lang="ts">
import { Pencil, Plus, Trash2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import HeaderDialog from "@/components/sites/HeaderDialog.vue";
import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { CARD, SECTION_LABEL } from "@/components/ui/styles";
import { useToast } from "@/composables/useToast";
import { IpcError, listResponseHeaders, removeResponseHeader, setResponseHeader } from "@/ipc/client";
import type { ResponseHeaderEntry, SiteEntry } from "@/ipc/types";

/**
 * The RESPONSE HEADERS card of the site inspector: this site's custom headers in
 * the order the proxy applies them, with Edit and Remove on each row and Add
 * header opening `HeaderDialog`. Headers are not part of `SiteEntry`, so the
 * card fetches its own and refetches after every change. Setting an existing
 * name replaces it, so an edit that renames a header sets the new name, then
 * removes the old one.
 */
const props = defineProps<{ site: SiteEntry }>();

const toast = useToast();

const headers = ref<ResponseHeaderEntry[]>([]);
const loading = ref(false);
/** Non-null while an IPC action is in flight; `add`, `edit:<name>` or `remove:<name>`. */
const busy = ref<string | null>(null);

const dialogOpen = ref(false);
const editing = ref<ResponseHeaderEntry | null>(null);

const siteHeaders = computed(() => headers.value.filter((h) => h.site === props.site.name));

/** Monotonic id of the latest `reload`, so a slow earlier response can't land after a site switch. */
let reloadId = 0;

async function reload(): Promise<void> {
  const id = ++reloadId;
  loading.value = true;
  try {
    const next = await listResponseHeaders();
    if (id === reloadId) headers.value = next;
  } catch (e) {
    if (id === reloadId) toast.error("Could not load response headers", (e as IpcError).message);
  } finally {
    if (id === reloadId) loading.value = false;
  }
}

/** A site switch refetches and closes an open dialog, so a staged add or edit
 *  can't be saved against the newly selected site. */
watch(
  () => props.site.name,
  () => {
    dialogOpen.value = false;
    editing.value = null;
    void reload();
  },
  { immediate: true },
);

function openAdd(): void {
  editing.value = null;
  dialogOpen.value = true;
}

function openEdit(h: ResponseHeaderEntry): void {
  editing.value = h;
  dialogOpen.value = true;
}

/** Run one IPC mutation behind the shared busy flag, toasting the outcome and
 *  refetching either way, since a rename may have applied its first step.
 *  Returns whether every step succeeded. */
async function run(key: string, fn: () => Promise<void>, ok: string): Promise<boolean> {
  if (busy.value !== null) return false;
  busy.value = key;
  try {
    await fn();
    toast.success(ok);
    return true;
  } catch (e) {
    toast.error("Header change failed", (e as IpcError).message);
    return false;
  } finally {
    busy.value = null;
    void reload();
  }
}

async function submit(name: string, value: string): Promise<void> {
  const old = editing.value;
  const site = props.site.name;
  const done = old
    ? await run(
        `edit:${old.name}`,
        async () => {
          await setResponseHeader(site, name, value);
          if (old.name.toLowerCase() !== name.toLowerCase()) await removeResponseHeader(site, old.name);
        },
        `Updated ${name}`,
      )
    : await run("add", () => setResponseHeader(site, name, value), `Set ${name}`);
  if (done) dialogOpen.value = false;
}

function remove(name: string): void {
  void run(`remove:${name}`, () => removeResponseHeader(props.site.name, name), `Removed ${name}`);
}

function rowBusy(name: string): boolean {
  return busy.value === `edit:${name}` || busy.value === `remove:${name}`;
}
</script>

<template>
  <section>
    <h3 :class="['mb-2 px-1', SECTION_LABEL]">Response headers</h3>
    <div :class="['divide-y overflow-hidden', CARD]" data-testid="headers-card">
      <div v-if="loading && siteHeaders.length === 0" class="flex h-11 items-center justify-center">
        <Spinner class="size-4" />
      </div>
      <div
        v-for="h in siteHeaders"
        :key="h.name"
        class="group flex h-11 items-center gap-3 px-4"
        data-testid="header-row"
      >
        <span class="flex min-w-0 flex-1 items-baseline gap-2 font-mono text-[13px]">
          <span class="shrink-0">{{ h.name }}</span>
          <span class="truncate text-muted-foreground" :title="h.value">{{ h.value }}</span>
        </span>
        <div
          class="flex shrink-0 items-center gap-0.5 opacity-60 transition-opacity focus-within:opacity-100 group-hover:opacity-100"
        >
          <Spinner v-if="rowBusy(h.name)" class="mx-1 size-3.5" />
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7"
            :disabled="busy !== null"
            :aria-label="`Edit header ${h.name}`"
            title="Edit header"
            @click="openEdit(h)"
          >
            <Pencil class="size-3.5" />
          </Button>
          <Button
            variant="ghost"
            size="icon"
            class="h-7 w-7 hover:text-destructive"
            :disabled="busy !== null"
            :aria-label="`Remove header ${h.name}`"
            title="Remove header"
            @click="remove(h.name)"
          >
            <Trash2 class="size-3.5" />
          </Button>
        </div>
      </div>
      <button
        type="button"
        class="flex h-11 w-full items-center gap-2 px-4 text-left text-[13px] font-medium text-brand transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring disabled:opacity-50"
        :disabled="busy !== null"
        @click="openAdd"
      >
        <Plus class="size-3.5" aria-hidden="true" /> Add header
      </button>
    </div>

    <HeaderDialog
      v-model:open="dialogOpen"
      :editing="editing"
      :busy="busy === 'add' || !!busy?.startsWith('edit:')"
      @submit="submit"
    />
  </section>
</template>
