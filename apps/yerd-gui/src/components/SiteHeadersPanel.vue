<script setup lang="ts">
import { FileText, Plus, Trash2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import EmptyState from "@/components/ui/EmptyState.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import { IpcError, listResponseHeaders, removeResponseHeader, setResponseHeader } from "@/ipc/client";
import type { ResponseHeaderEntry, SiteEntry } from "@/ipc/types";

const props = defineProps<{
  site: SiteEntry;
}>();

const toast = useToast();

/** This panel owns its own data end to end, like the routing panel: response
 *  headers are not part of `SiteEntry`, so it refetches itself after every
 *  mutation and emits nothing. */
const headers = ref<ResponseHeaderEntry[]>([]);
const loading = ref(false);

/** Non-null while an IPC action is in flight; the value is a per-action key
 *  (`set:<name>` / `remove:<name>`) so exactly one row shows a spinner while
 *  every control is disabled. */
const busy = ref<string | null>(null);

const newName = ref("");
const newValue = ref("");

/** This site's headers in their configured order, which is the order the proxy
 *  applies them in. */
const siteHeaders = computed(() => headers.value.filter((h) => h.site === props.site.name));

const canSet = computed(
  () => newName.value.trim() !== "" && newValue.value.trim() !== "" && busy.value === null,
);

/** Monotonic id of the most recent `reload` call, so a slow earlier response
 *  cannot overwrite a newer one after a quick site switch. */
let reloadId = 0;

async function reload(): Promise<void> {
  const id = ++reloadId;
  loading.value = true;
  try {
    const next = await listResponseHeaders();
    if (id !== reloadId) return;
    headers.value = next;
  } catch (e) {
    if (id !== reloadId) return;
    toast.error("Could not load response headers", (e as IpcError).message);
  } finally {
    if (id === reloadId) loading.value = false;
  }
}

watch(
  () => props.site.name,
  () => {
    newName.value = "";
    newValue.value = "";
    void reload();
  },
  { immediate: true },
);

/** Run one IPC mutation behind the shared busy flag, toasting the outcome and
 *  refetching on success. The daemon validates header names and values, so a
 *  rejection arrives as a thrown `IpcError`. */
async function run(key: string, fn: () => Promise<void>, ok: string): Promise<void> {
  if (busy.value !== null) return;
  busy.value = key;
  try {
    await fn();
    toast.success(ok);
    await reload();
  } catch (e) {
    toast.error("Header change failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

function set(): void {
  const name = newName.value.trim();
  const value = newValue.value.trim();
  if (!canSet.value) return;
  void run(
    `set:${name}`,
    async () => {
      await setResponseHeader(props.site.name, name, value);
      if (newName.value.trim() === name) newName.value = "";
      if (newValue.value.trim() === value) newValue.value = "";
    },
    `Set ${name}`,
  );
}

function remove(name: string): void {
  void run(`remove:${name}`, () => removeResponseHeader(props.site.name, name), `Removed ${name}`);
}
</script>

<template>
  <div class="space-y-4">
    <div v-if="loading && siteHeaders.length === 0" class="flex justify-center py-4">
      <Spinner class="size-5" />
    </div>

    <ul v-else-if="siteHeaders.length" class="space-y-1.5">
      <li
        v-for="h in siteHeaders"
        :key="h.name"
        class="flex items-center gap-2 rounded-md border bg-card px-3 py-2"
      >
        <span class="shrink-0 font-mono text-sm">{{ h.name }}:</span>
        <span class="truncate font-mono text-sm text-muted-foreground">{{ h.value }}</span>

        <div class="ml-auto flex shrink-0 items-center gap-1">
          <Spinner v-if="busy === `remove:${h.name}`" class="size-4" />
          <Button
            variant="ghost"
            size="icon"
            :disabled="busy !== null"
            :title="`Remove ${h.name}`"
            :aria-label="`Remove header ${h.name}`"
            @click="remove(h.name)"
          >
            <Trash2 class="size-4" />
          </Button>
        </div>
      </li>
    </ul>

    <EmptyState v-else :icon="FileText" title="No custom response headers for this site." />

    <div>
      <label for="add-header-name" class="text-sm font-medium">Set a header</label>
      <div class="mt-2 flex gap-2">
        <Input
          id="add-header-name"
          v-model="newName"
          placeholder="Access-Control-Allow-Origin"
          aria-label="Header name"
          class="min-w-0 flex-1"
          @keydown.enter="set"
        />
        <Input
          id="add-header-value"
          v-model="newValue"
          placeholder="https://app.test"
          aria-label="Header value"
          class="min-w-0 flex-1"
          @keydown.enter="set"
        />
        <Button :disabled="!canSet" @click="set">
          <Spinner v-if="busy?.startsWith('set:')" class="size-4" />
          <Plus v-else class="size-4" /> Set
        </Button>
      </div>
      <InfoBanner class="mt-4">
        Headers are added to every response for this site, including errors and redirects, but not
        to responses from proxy path rules. Setting a name that is already listed replaces it, and
        a header with the same name sent by the app is replaced too.
      </InfoBanner>
    </div>
  </div>
</template>
