<script setup lang="ts">
import { Plus, Trash2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import { addRouteRule, IpcError, listRoutes, removeRouteRule } from "@/ipc/client";
import type { RouteRuleEntry, SiteEntry } from "@/ipc/types";

const props = defineProps<{
  site: SiteEntry;
}>();

const toast = useToast();

/** This panel owns its own data end to end. Routing rules are not part of
 *  `SiteEntry`, so unlike the domains panel there is nothing for the parent to
 *  reload - it refetches itself after every mutation and emits nothing. */
const rules = ref<RouteRuleEntry[]>([]);
const loading = ref(false);

/** Non-null while an IPC action is in flight; the value is a per-action key
 *  (`add:<prefix>` / `remove:<prefix>`) so exactly one row shows a spinner while
 *  every control is disabled. */
const busy = ref<string | null>(null);

const newPrefix = ref("");
const newTarget = ref("");

const siteRules = computed(() =>
  rules.value.filter((r) => r.site === props.site.name).sort((a, b) => a.prefix.localeCompare(b.prefix)),
);

const prefixError = computed(() => {
  const v = newPrefix.value.trim();
  if (v === "") return null;
  return v.startsWith("/") ? null : "A prefix must begin with '/'.";
});

const canAdd = computed(
  () =>
    newPrefix.value.trim() !== "" &&
    newTarget.value.trim() !== "" &&
    prefixError.value === null &&
    busy.value === null,
);

async function reload(): Promise<void> {
  loading.value = true;
  try {
    rules.value = await listRoutes();
  } catch (e) {
    toast.error("Could not load routing rules", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

// Refetch when the panel is pointed at a different site: the sidebar keeps a
// single instance and swaps the `site` prop rather than remounting.
watch(
  () => props.site.name,
  () => {
    newPrefix.value = "";
    newTarget.value = "";
    void reload();
  },
  { immediate: true },
);

/** Run one IPC mutation behind the shared busy flag, toasting the outcome and
 *  refetching on success. Daemon-authoritative rejections (a duplicate prefix,
 *  an absolute or `..`-containing target) arrive as a thrown `IpcError`. */
async function run(key: string, fn: () => Promise<void>, ok: string): Promise<void> {
  if (busy.value !== null) return;
  busy.value = key;
  try {
    await fn();
    toast.success(ok);
    await reload();
  } catch (e) {
    toast.error("Routing change failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

function add(): void {
  const prefix = newPrefix.value.trim();
  const target = newTarget.value.trim();
  if (!canAdd.value) return;
  void run(
    `add:${prefix}`,
    async () => {
      await addRouteRule(props.site.name, prefix, target);
      newPrefix.value = "";
      newTarget.value = "";
    },
    `Added ${prefix} → ${target}`,
  );
}

function remove(prefix: string): void {
  void run(`remove:${prefix}`, () => removeRouteRule(props.site.name, prefix), `Removed ${prefix}`);
}
</script>

<template>
  <div class="space-y-4">
    <div v-if="loading && siteRules.length === 0" class="flex justify-center py-4">
      <Spinner class="size-5" />
    </div>

    <ul v-else-if="siteRules.length" class="space-y-1.5">
      <li
        v-for="r in siteRules"
        :key="r.prefix"
        class="flex items-center gap-2 rounded-md border bg-card px-3 py-2"
      >
        <span class="truncate font-mono text-sm">{{ r.prefix }}</span>
        <span class="shrink-0 text-muted-foreground">&rarr;</span>
        <span class="truncate font-mono text-sm text-muted-foreground">{{ r.target }}</span>

        <div class="ml-auto flex shrink-0 items-center gap-1">
          <Spinner v-if="busy === `remove:${r.prefix}`" class="size-4" />
          <Button
            variant="ghost"
            size="icon"
            :disabled="busy !== null"
            :title="`Remove ${r.prefix}`"
            :aria-label="`Remove route ${r.prefix}`"
            @click="remove(r.prefix)"
          >
            <Trash2 class="size-4" />
          </Button>
        </div>
      </li>
    </ul>

    <p v-else class="text-sm text-muted-foreground">No routing rules for this site.</p>

    <div>
      <label for="add-route-prefix" class="text-sm font-medium">Add a rule</label>
      <div class="mt-2 flex gap-2">
        <Input
          id="add-route-prefix"
          v-model="newPrefix"
          placeholder="/api"
          aria-label="Route prefix"
          class="min-w-0 flex-1"
          @keydown.enter="add"
        />
        <Input
          id="add-route-target"
          v-model="newTarget"
          placeholder="api/index.php"
          aria-label="Route target"
          class="min-w-0 flex-1"
          @keydown.enter="add"
        />
        <Button :disabled="!canAdd" @click="add">
          <Spinner v-if="busy?.startsWith('add:')" class="size-4" />
          <Plus v-else class="size-4" /> Add
        </Button>
      </div>
      <p v-if="prefixError" class="mt-1 text-xs text-destructive">{{ prefixError }}</p>
      <p v-else class="mt-1 text-xs text-muted-foreground">
        Requests under the prefix that match no real file are handled by the target, a path
        relative to this site's web root. A <code class="font-mono">.php</code> target runs as a
        nested front controller; anything else is served as a static file.
      </p>
    </div>

    <p class="border-t pt-4 text-xs text-muted-foreground">
      A site whose web root holds an <code class="font-mono">index.html</code> and no
      <code class="font-mono">index.php</code> already routes deep links to it automatically, with
      no rule needed.
    </p>
  </div>
</template>
