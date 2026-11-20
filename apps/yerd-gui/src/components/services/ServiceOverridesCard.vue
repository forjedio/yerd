<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Plus, SlidersHorizontal, X } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import {
  IpcError,
  serviceOverrides,
  setServiceOverride,
  unsetServiceOverride,
} from "@/ipc/client";
import type { ServiceStatus } from "@/ipc/types";
import { FIELD } from "@/components/services/styles";
import { CARD } from "@/components/ui/styles";
import { cn } from "@/lib/utils";

/**
 * The inspector's Config overrides card. It owns the override map end to end:
 * overrides are not part of `ServiceStatus`, so it fetches them itself, refetches
 * after every change and emits nothing. The daemon is the authority on what a
 * service accepts, so its rejections are shown verbatim.
 */
const props = defineProps<{ service: ServiceStatus }>();

const toast = useToast();

const overrides = ref<Record<string, string>>({});
const loading = ref(false);
/** The in-flight action (`add:<name>` / `remove:<name>`), so one row spins while
 *  every control is disabled. */
const busy = ref<string | null>(null);
const adding = ref(false);
const newKey = ref("");
const newValue = ref("");

const entries = computed(() =>
  Object.entries(overrides.value).sort(([a], [b]) => a.localeCompare(b)),
);

const canAdd = computed(
  () => newKey.value.trim() !== "" && newValue.value.trim() !== "" && busy.value === null,
);

/** Monotonic id of the latest `reload`; switching service can leave two fetches
 *  in flight and only the newest may write. */
let reloadId = 0;

async function reload(): Promise<void> {
  const id = ++reloadId;
  const s = props.service;
  loading.value = true;
  try {
    const next = await serviceOverrides(s.service);
    if (id !== reloadId) return;
    overrides.value = next;
  } catch (e) {
    if (id !== reloadId) return;
    toast.error("Couldn't load overrides", (e as IpcError).message);
  } finally {
    if (id === reloadId) loading.value = false;
  }
}

function resetAddRow(): void {
  adding.value = false;
  newKey.value = "";
  newValue.value = "";
}

watch(
  () => props.service.service,
  () => {
    resetAddRow();
    overrides.value = {};
    void reload();
  },
  { immediate: true },
);

async function run(key: string, fn: () => Promise<void>): Promise<boolean> {
  const s = props.service;
  if (busy.value !== null) return false;
  busy.value = key;
  try {
    await fn();
    toast.success(`Saved - restart ${s.display_name} to apply`);
    await reload();
    return true;
  } catch (e) {
    toast.error("Override change failed", (e as IpcError).message);
    return false;
  } finally {
    busy.value = null;
  }
}

async function add(): Promise<void> {
  const s = props.service;
  const key = newKey.value.trim();
  const value = newValue.value.trim();
  if (!canAdd.value) return;
  if (await run(`add:${key}`, () => setServiceOverride(s.service, key, value))) resetAddRow();
}

function remove(key: string): void {
  const s = props.service;
  void run(`remove:${key}`, () => unsetServiceOverride(s.service, key));
}
</script>

<template>
  <section :class="cn(CARD, 'p-4')" data-testid="overrides-card">
    <h3 class="flex items-center gap-2 text-sm font-semibold">
      <SlidersHorizontal class="size-4 text-muted-foreground" aria-hidden="true" />
      Config overrides
    </h3>
    <p class="mt-1.5 text-xs leading-relaxed text-muted-foreground">
      Written into {{ service.display_name }}'s config on every start. Restart
      {{ service.display_name }} to apply a change.
    </p>

    <div v-if="loading && entries.length === 0" class="flex justify-center py-4">
      <Spinner class="size-4" />
    </div>
    <ul v-else-if="entries.length" class="mt-3 divide-y rounded-md border bg-background">
      <li v-for="[key, value] in entries" :key="key" class="flex h-9 items-center gap-3 pl-2.5 pr-1">
        <span class="min-w-0 flex-1 truncate font-mono text-xs">{{ key }}</span>
        <span class="max-w-[45%] truncate font-mono text-xs text-muted-foreground">{{ value }}</span>
        <Spinner v-if="busy === `remove:${key}`" class="size-3.5 shrink-0" />
        <button
          v-else
          type="button"
          class="flex size-6 shrink-0 items-center justify-center rounded text-faint transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
          :disabled="busy !== null"
          :aria-label="`Remove override ${key}`"
          :title="`Remove ${key}`"
          @click="remove(key)"
        >
          <X class="size-3.5" />
        </button>
      </li>
    </ul>
    <p v-else class="mt-3 text-xs text-faint">No overrides yet.</p>

    <div v-if="adding" class="mt-3 space-y-2" @keydown.esc.stop="resetAddRow">
      <div class="flex gap-2">
        <input
          v-model="newKey"
          placeholder="max_connections"
          aria-label="Override name"
          spellcheck="false"
          :disabled="busy !== null"
          :class="cn(FIELD, 'min-w-0 flex-1')"
          @keydown.enter="add"
        />
        <input
          v-model="newValue"
          placeholder="500"
          aria-label="Override value"
          spellcheck="false"
          :disabled="busy !== null"
          :class="cn(FIELD, 'w-24')"
          @keydown.enter="add"
        />
      </div>
      <div class="flex justify-end gap-2">
        <Button variant="ghost" size="sm" :disabled="busy !== null" @click="resetAddRow">Cancel</Button>
        <Button size="sm" :disabled="!canAdd" @click="add">
          <Spinner v-if="busy?.startsWith('add:')" class="size-4" />
          Add
        </Button>
      </div>
    </div>
    <button
      v-else
      type="button"
      class="mt-3 inline-flex items-center gap-1.5 rounded text-[13px] font-medium text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
      @click="adding = true"
    >
      <Plus class="size-3.5" aria-hidden="true" /> Add override
    </button>
  </section>
</template>
