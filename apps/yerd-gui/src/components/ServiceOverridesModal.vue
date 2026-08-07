<script setup lang="ts">
import { Plus, Trash2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import {
  IpcError,
  serviceOverrides,
  setServiceOverride,
  unsetServiceOverride,
} from "@/ipc/client";
import type { ServiceStatus } from "@/ipc/types";

const props = defineProps<{
  open: boolean;
  /** The row the modal was opened from; null before the first open. */
  service: ServiceStatus | null;
}>();

const emit = defineEmits<{
  (e: "update:open", open: boolean): void;
}>();

const toast = useToast();

/** This modal owns its data end to end: overrides are not part of
 *  `ServiceStatus`, so there is nothing for the Services view to reload - it
 *  refetches itself after every mutation and emits nothing but its open state. */
const overrides = ref<Record<string, string>>({});
const loading = ref(false);

/** Non-null while an IPC action is in flight; the value is a per-action key
 *  (`add:<name>` / `remove:<name>`) so exactly one row shows a spinner while
 *  every control is disabled. */
const busy = ref<string | null>(null);

const newKey = ref("");
const newValue = ref("");

const entries = computed(() =>
  Object.entries(overrides.value).sort(([a], [b]) => a.localeCompare(b)),
);

const canAdd = computed(
  () => newKey.value.trim() !== "" && newValue.value.trim() !== "" && busy.value === null,
);

/** Monotonic id of the most recent `reload` call. Reopening on a different
 *  service can leave two fetches in flight; only the newest may touch
 *  `overrides`, `loading` or raise a toast. */
let reloadId = 0;

async function reload(): Promise<void> {
  const s = props.service;
  if (!s) return;
  const id = ++reloadId;
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

// Load on open, and again when the view points the modal at another service.
watch(
  () => [props.open, props.service?.service] as const,
  ([isOpen]) => {
    if (!isOpen) return;
    newKey.value = "";
    newValue.value = "";
    overrides.value = {};
    void reload();
  },
  { immediate: true },
);

/** Run one IPC mutation behind the shared busy flag, then refetch. The daemon
 *  is the authority on what a service accepts, so its rejection (a reserved
 *  directive and its hint, a bad name/value shape, a service with no override
 *  support) is surfaced verbatim. */
async function run(key: string, fn: () => Promise<void>): Promise<void> {
  const s = props.service;
  if (!s || busy.value !== null) return;
  busy.value = key;
  try {
    await fn();
    toast.success(`Saved - restart ${s.display_name} to apply`);
    await reload();
  } catch (e) {
    toast.error("Override change failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

function add(): void {
  const s = props.service;
  const key = newKey.value.trim();
  const value = newValue.value.trim();
  if (!s || !canAdd.value) return;
  void run(`add:${key}`, async () => {
    await setServiceOverride(s.service, key, value);
    newKey.value = "";
    newValue.value = "";
  });
}

function remove(key: string): void {
  const s = props.service;
  if (!s) return;
  void run(`remove:${key}`, () => unsetServiceOverride(s.service, key));
}
</script>

<template>
  <Modal
    :open="open"
    size="lg"
    :title="`${service?.display_name ?? 'Service'} overrides`"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <div class="space-y-4">
      <p class="text-sm text-muted-foreground">
        Settings Yerd writes into {{ service?.display_name ?? "the service" }}'s generated
        config on every start. Yerd refuses directives it manages itself, such as the port
        and data directory.
      </p>

      <div v-if="loading && entries.length === 0" class="flex justify-center py-4">
        <Spinner class="size-5" />
      </div>

      <ul v-else-if="entries.length" class="space-y-1.5">
        <li
          v-for="[key, value] in entries"
          :key="key"
          class="flex items-center gap-2 rounded-md border bg-card px-3 py-2"
        >
          <span class="truncate font-mono text-sm">{{ key }}</span>
          <span class="shrink-0 text-muted-foreground">=</span>
          <span class="truncate font-mono text-sm text-muted-foreground">{{ value }}</span>

          <div class="ml-auto flex shrink-0 items-center gap-1">
            <Spinner v-if="busy === `remove:${key}`" class="size-4" />
            <Button
              variant="ghost"
              size="icon"
              :disabled="busy !== null"
              :title="`Remove ${key}`"
              :aria-label="`Remove override ${key}`"
              @click="remove(key)"
            >
              <Trash2 class="size-4" />
            </Button>
          </div>
        </li>
      </ul>

      <p v-else class="text-sm text-muted-foreground">No overrides for this service.</p>

      <div>
        <label for="add-override-name" class="text-sm font-medium">Add an override</label>
        <div class="mt-2 flex gap-2">
          <Input
            id="add-override-name"
            v-model="newKey"
            placeholder="max_connections"
            aria-label="Override name"
            :disabled="busy !== null"
            class="min-w-0 flex-1"
            @keydown.enter="add"
          />
          <Input
            id="add-override-value"
            v-model="newValue"
            placeholder="500"
            aria-label="Override value"
            :disabled="busy !== null"
            class="min-w-0 flex-1"
            @keydown.enter="add"
          />
          <Button :disabled="!canAdd" @click="add">
            <Spinner v-if="busy?.startsWith('add:')" class="size-4" />
            <Plus v-else class="size-4" /> Add
          </Button>
        </div>
        <p class="mt-1 text-xs text-muted-foreground">
          Changes are saved straight away but only reach the engine on its next restart.
        </p>
      </div>
    </div>

    <template #footer="{ close }">
      <Button variant="ghost" @click="close">Close</Button>
    </template>
  </Modal>
</template>
