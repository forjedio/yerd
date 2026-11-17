<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { useRouter } from "vue-router";

import Button from "@/components/ui/Button.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useOperations } from "@/composables/useOperations";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { clearNodeVersion, IpcError, listNode, nodeSite, setNodeVersion } from "@/ipc/client";
import type { NodeSiteResponse, NodeSource, SiteEntry } from "@/ipc/types";

const props = defineProps<{
  site: SiteEntry;
}>();

const toast = useToast();
const router = useRouter();
const operations = useOperations();

/** Shares the Node view's cache, so an install there updates these options. */
const { data: releases } = useResource("node", listNode);

/** This panel owns its data like the headers panel: Node status is not part of
 *  `SiteEntry`, so it fetches per site and refetches after every change. */
const status = ref<NodeSiteResponse | null>(null);
const loading = ref(false);
const busy = ref(false);
/** Bumped after a failed change so the select re-renders with the saved value. */
const selectKey = ref(0);

const installing = computed(() =>
  operations.active.value.some((o) => o.id.startsWith("node-install:")),
);

const noneInstalled = computed(() => releases.value !== null && releases.value.length === 0);

const DEFAULT_OPTION = { value: "", label: "Default" };

/** Installed releases, plus the saved preference when it is not one of them
 *  (a partial selector such as `22`, or a release since removed). */
const options = computed(() => {
  const installed = (releases.value ?? []).map((r) => r.version);
  const pref = status.value?.preference;
  const saved = pref && !installed.includes(pref) ? [{ value: pref, label: `${pref} (saved)` }] : [];
  return [DEFAULT_OPTION, ...installed.map((v) => ({ value: v, label: v })), ...saved];
});

const FILE_NAMES: Partial<Record<NodeSource, string>> = {
  nvmrc: ".nvmrc",
  "node-version": ".node-version",
};

const projectFileName = computed(() => {
  const kind = status.value?.project_file?.kind;
  return kind ? (FILE_NAMES[kind] ?? kind) : "";
});

const shadowed = computed(() => !!status.value?.project_file && !!status.value?.preference);

/** Monotonic id of the latest `reload`, so a slow response for a previous site
 *  cannot overwrite the current one after a quick switch. */
let reloadId = 0;

async function reload(): Promise<void> {
  const id = ++reloadId;
  const name = props.site.name;
  loading.value = true;
  try {
    const next = await nodeSite(name);
    if (id !== reloadId) return;
    status.value = next;
  } catch (e) {
    if (id !== reloadId) return;
    status.value = null;
    toast.error("Could not load the site's Node version", (e as IpcError).message);
  } finally {
    if (id === reloadId) loading.value = false;
  }
}

watch(
  () => props.site.name,
  () => {
    status.value = null;
    void reload();
  },
  { immediate: true },
);

/** A Node install can make a missing release resolvable, so refetch when one ends. */
watch(installing, (now, was) => {
  if (was && !now) void reload();
});

async function change(version: string): Promise<void> {
  if (busy.value || installing.value) return;
  const name = props.site.name;
  busy.value = true;
  try {
    if (version === "") {
      await clearNodeVersion(name);
      toast.success(`Cleared the Node preference for ${name}`);
    } else {
      await setNodeVersion(version, name);
      toast.success(`${name} now prefers Node ${version}`);
    }
    await reload();
  } catch (e) {
    selectKey.value += 1;
    toast.error("Couldn't change the Node version", (e as IpcError).message);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div
    class="grid grid-cols-[auto_1fr] items-center gap-x-4 gap-y-2 px-3 py-3"
    data-testid="site-node-panel"
  >
    <dt class="text-sm font-medium">Node version</dt>
    <dd class="flex min-w-0 items-center justify-end gap-2">
      <Spinner v-if="loading || busy" class="size-4" />
      <Select
        :key="selectKey"
        :model-value="status?.preference ?? ''"
        :options="options"
        aria-label="Site Node version"
        :disabled="busy || installing || (noneInstalled && !status?.preference)"
        @update:model-value="change"
      />
    </dd>

    <dd class="col-span-2 space-y-2">
      <p v-if="noneInstalled" class="text-xs text-muted-foreground">
        No Node release is installed yet.
      </p>

      <InfoBanner v-if="shadowed" variant="warning" data-testid="site-node-precedence">
        This project's <code>{{ projectFileName }}</code> ({{ status?.project_file?.selector }})
        takes precedence over the saved preference ({{ status?.preference }}).
      </InfoBanner>

      <div v-if="status?.error" class="space-y-1" data-testid="site-node-error">
        <p class="text-sm text-destructive">{{ status.error }}</p>
        <Button variant="link" size="sm" class="h-auto p-0" @click="router.push('/node')">
          Manage Node versions
        </Button>
      </div>

      <p class="text-xs text-muted-foreground" data-testid="site-node-hint">
        <code>node</code> and <code>npm</code> use the global default; run
        <code>yerd exec node …</code> or <code>yerd exec npm …</code> in the project folder to use
        this site's version.
      </p>
    </dd>
  </div>
</template>
