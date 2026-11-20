<script setup lang="ts">
import { computed, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";

import Button from "@/components/ui/Button.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useOperations } from "@/composables/useOperations";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import {
  clearNodeVersion,
  installNodeWithProgress,
  IpcError,
  listNode,
  nodeSite,
  setNodeVersion,
} from "@/ipc/client";
import type { NodeSiteResponse, NodeSource, SiteEntry } from "@/ipc/types";

/**
 * The inspector's Node version row: a segmented "Default" plus the installed
 * releases (a select when there are many, or when the saved preference is not
 * an installed release), with the precedence banner, the missing-release error
 * and install progress beneath it. Emits `changed` after a successful change or
 * install so the list's Node column can refresh.
 */
const props = defineProps<{
  site: SiteEntry;
}>();

const emit = defineEmits<{ changed: [] }>();

const toast = useToast();
const router = useRouter();
const operations = useOperations();

/** Shares the Node view's cache, so an install there updates these options. */
const { data: releases, refresh: refreshReleases } = useResource("node", listNode);

/** This panel owns its data like the headers panel: Node status is not part of
 *  `SiteEntry`, so it fetches per site and refetches after every change. */
const status = ref<NodeSiteResponse | null>(null);
const loading = ref(false);
/** The site whose Node change is in flight, so switching sites mid-write
 *  leaves the next site's picker usable. */
const busySite = ref<string | null>(null);
const busy = computed(() => busySite.value === props.site.name);
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
 *  cannot overwrite the current one after a quick switch. Unmounting bumps it
 *  too, so a late reply or rejection from a closed panel is ignored. */
let reloadId = 0;
let disposed = false;

onUnmounted(() => {
  disposed = true;
  reloadId += 1;
});

async function reload(): Promise<void> {
  if (disposed) return;
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

/** The selector the site resolved to (project file, preference, or the global
 *  default) when no installed release matches it, as reported by the daemon. */
const missingSelector = computed(() => status.value?.missing ?? null);

/** The latest install log line, tagged with the site that started the install
 *  so it isn't shown under another site after a switch. */
const installLine = ref<{ site: string; line: string } | null>(null);
const installProgress = computed(() =>
  installLine.value?.site === props.site.name ? installLine.value.line : "",
);

/** Install the missing release in place, as the Node page would; the
 *  `installing` watch reloads this site's status once it ends. */
async function installMissing(): Promise<void> {
  const version = missingSelector.value;
  if (!version || busy.value || installing.value) return;
  const opId = `node-install:${version}`;
  const site = props.site.name;
  operations.begin({ id: opId, kind: "tool-install", label: `Installing Node ${version}` });
  installLine.value = null;
  try {
    await installNodeWithProgress(version, false, (lines) => {
      const latest = lines[lines.length - 1];
      if (!latest) return;
      installLine.value = { site, line: latest };
      operations.update(opId, { detail: latest });
    });
    toast.success(`Installed Node ${version}`);
    emit("changed");
  } catch (e) {
    toast.error(`Install of Node ${version} failed`, (e as IpcError).message);
  } finally {
    operations.end(opId);
    installLine.value = null;
    await refreshReleases({ force: true });
  }
}

async function change(version: string): Promise<void> {
  if (busy.value || installing.value) return;
  const name = props.site.name;
  busySite.value = name;
  try {
    if (version === "") {
      await clearNodeVersion(name);
      toast.success(`Cleared the Node preference for ${name}`);
    } else {
      await setNodeVersion(version, name);
      toast.success(`${name} now prefers Node ${version}`);
    }
    await reload();
    emit("changed");
  } catch (e) {
    selectKey.value += 1;
    toast.error("Couldn't change the Node version", (e as IpcError).message);
  } finally {
    if (busySite.value === name) busySite.value = null;
  }
}

/** Above this many releases the segmented control would overflow the row. */
const SEGMENTED_MAX_RELEASES = 4;

/** The segmented control's stand-in for "no preference" (reka radios need a non-empty value). */
const DEFAULT_SEGMENT = "default";

const installedVersions = computed(() => (releases.value ?? []).map((r) => r.version));

const useSelect = computed(() => {
  const pref = status.value?.preference;
  return (
    installedVersions.value.length > SEGMENTED_MAX_RELEASES ||
    (!!pref && !installedVersions.value.includes(pref))
  );
});

/** Majors when they are unambiguous ("18 20 22", as drawn), else full versions. */
const segments = computed(() => {
  const majors = installedVersions.value.map((v) => v.split(".")[0] ?? v);
  const unique = new Set(majors).size === majors.length;
  return [
    { value: DEFAULT_SEGMENT, label: "Default" },
    ...installedVersions.value.map((v, i) => ({ value: v, label: unique ? (majors[i] ?? v) : v })),
  ];
});

function changeSegment(value: string): void {
  void change(value === DEFAULT_SEGMENT ? "" : value);
}

const subtitle = computed(() => {
  if (noneInstalled.value) return "No Node release installed";
  const s = status.value;
  if (!s?.effective) return "";
  const builds = `builds use Node ${s.effective}`;
  if (s.source === "site") return `Saved preference · ${builds}`;
  if (s.source === "nvmrc" || s.source === "node-version") {
    return `Project file ${projectFileName.value} · ${builds}`;
  }
  return `Global default · ${builds}`;
});

const HINT =
  "node and npm use the global default; run yerd exec node … or yerd exec npm … in the project folder to use this site's version.";
</script>

<template>
  <div class="px-4 py-2.5" data-testid="site-node-panel">
    <div class="flex min-h-9 items-center justify-between gap-4">
      <div class="min-w-0" :title="HINT" data-testid="site-node-hint">
        <p class="text-[13px] font-semibold">Node version</p>
        <p class="truncate text-[11px] text-faint" data-testid="site-node-subtitle">
          {{ subtitle }}
        </p>
      </div>
      <div class="flex shrink-0 items-center gap-2">
        <Spinner v-if="loading || busy" class="size-4" />
        <Select
          v-if="useSelect"
          :key="selectKey"
          :model-value="status?.preference ?? ''"
          :options="options"
          aria-label="Site Node version"
          :disabled="busy || installing || (noneInstalled && !status?.preference)"
          @update:model-value="change"
        />
        <SegmentedControl
          v-else
          :key="selectKey"
          :model-value="status?.preference || DEFAULT_SEGMENT"
          :options="segments"
          aria-label="Site Node version"
          manual-activation
          :disabled="busy || installing || noneInstalled"
          @update:model-value="changeSegment"
        />
      </div>
    </div>

    <div
      v-if="shadowed || status?.error"
      class="mt-2 space-y-2"
    >
      <InfoBanner v-if="shadowed" variant="warning" data-testid="site-node-precedence">
        This project's <code>{{ projectFileName }}</code> ({{ status?.project_file?.selector }})
        takes precedence over the saved preference ({{ status?.preference }}).
      </InfoBanner>

      <div v-if="status?.error" class="space-y-1" data-testid="site-node-error">
        <p class="text-xs text-destructive">{{ status.error }}</p>
        <div class="flex flex-wrap items-center gap-2">
          <Button
            v-if="missingSelector"
            size="sm"
            :disabled="busy || installing"
            data-testid="site-node-install"
            @click="installMissing"
          >
            <Spinner v-if="installing" class="size-3.5" />
            Install Node {{ missingSelector }}
          </Button>
          <Button variant="link" size="sm" class="h-auto p-0" @click="router.push('/node')">
            Manage Node versions
          </Button>
        </div>
        <p v-if="installProgress" class="truncate text-xs text-muted-foreground">
          {{ installProgress }}
        </p>
      </div>
    </div>
  </div>
</template>
