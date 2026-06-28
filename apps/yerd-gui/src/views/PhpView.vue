<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import {
  Download,
  Info,
  MoreHorizontal,
  RefreshCw,
  RotateCw,
  Star,
  Trash2,
} from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import StatusPill from "@/components/StatusPill.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useOperations } from "@/composables/useOperations";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import {
  availablePhp,
  checkPhpUpdates,
  installPhpWithProgress,
  IpcError,
  listPhp,
  restartAllPhp,
  restartPhp,
  setDefaultPhp,
  setPhpSettings,
  uninstallPhp,
  updatePhp,
} from "@/ipc/client";
import type { PhpPoolStatus, PhpUpdate, PhpVersion } from "@/ipc/types";
import { humaniseBytes, poolStateLabel, poolStateTone } from "@/lib/utils";

const toast = useToast();
const { report, refresh } = useDaemon();
const operations = useOperations();

// Cached SWR resource: revisits render the installed-versions table instantly
// and revalidate underneath instead of flashing a spinner each time.
const { data, loading, error, refresh: reloadPhp, mutate } = useResource("php", listPhp);
const installed = computed<PhpVersion[]>(() => data.value?.installed ?? []);
const defaultVersion = computed<PhpVersion | null>(() => data.value?.default ?? null);
const updates = computed<PhpUpdate[]>(() => data.value?.updates ?? []);
const busy = ref<string | null>(null); // a key naming the in-flight long op

// Surface a cold-load failure as a toast (no AsyncState here), masked once
// there's cached data so a background revalidation stays silent.
watch(error, (e) => {
  if (e && !data.value) toast.error("Couldn't load PHP versions", e.message);
});

// An install is tracked in the global operations registry so it persists and
// stays visible (here and in the SideNav) across navigation.
const installing = computed(() => operations.active.value.some((o) => o.kind === "php-install"));
const installDetail = computed(
  () => operations.active.value.find((o) => o.kind === "php-install")?.detail ?? "",
);

// Live FPM state, keyed by version, from the shared status poll.
const poolByVersion = computed<Record<string, PhpPoolStatus>>(() => {
  const map: Record<string, PhpPoolStatus> = {};
  for (const p of report.value?.php ?? []) map[p.version] = p;
  return map;
});

const updateByVersion = computed<Record<string, PhpUpdate>>(() => {
  const map: Record<string, PhpUpdate> = {};
  for (const u of updates.value) map[u.version] = u;
  return map;
});

const hasUpdates = computed(() => updates.value.length > 0);

// ── global PHP ini settings ──
// Text fields plus an On/Off select for display_errors. A blank field means
// "use PHP's default" (the daemon removes the key).
const TEXT_SETTINGS = [
  {
    key: "memory_limit",
    label: "Memory limit",
    placeholder: "512M",
    hint: "Most memory one script may use. Size like 256M, 512M or 2G (use G, not GB). -1 means unlimited.",
  },
  {
    key: "max_execution_time",
    label: "Max execution time (s)",
    placeholder: "60",
    hint: "How long a script may run before it's stopped. Whole seconds, e.g. 60. 0 means no limit.",
  },
  {
    key: "max_input_time",
    label: "Max input time (s)",
    placeholder: "60",
    hint: "How long a script may spend reading request data (POST and uploads). Whole seconds, e.g. 60.",
  },
  {
    key: "max_file_uploads",
    label: "Max file uploads",
    placeholder: "20",
    hint: "How many files may be uploaded in one request. Whole number, e.g. 20.",
  },
  {
    key: "upload_max_filesize",
    label: "Upload max filesize",
    placeholder: "100M",
    hint: "Largest single uploaded file. Size like 8M, 100M or 1G (use G, not GB).",
  },
  {
    key: "post_max_size",
    label: "Post max size",
    placeholder: "100M",
    hint: "Largest POST body; set this at or above the upload size. Size like 8M, 100M or 1G (use G, not GB).",
  },
  {
    key: "error_reporting",
    label: "Error reporting",
    placeholder: "E_ALL",
    hint: "Which error levels PHP reports. An integer or a constant expression, e.g. E_ALL or E_ALL & ~E_DEPRECATED.",
  },
] as const;

const DISPLAY_ERRORS_HINT =
  "Whether PHP shows errors in the page output. On is handy in development; Off is safer in production.";

const DISPLAY_ERRORS_OPTIONS = [
  { value: "", label: "- default -" },
  { value: "On", label: "On" },
  { value: "Off", label: "Off" },
] as const;

const settingsForm = ref<Record<string, string>>({});

function applySettings(settings: Record<string, string> | undefined): void {
  const next: Record<string, string> = {};
  for (const s of TEXT_SETTINGS) next[s.key] = settings?.[s.key] ?? "";
  next.display_errors = settings?.display_errors ?? "";
  settingsForm.value = next;
}

// Seed the settings form once, on the first non-null data (synchronously on a
// warm-cache revisit via immediate:true, so the inputs never flash empty).
// Deliberately NOT re-seeded on later data changes - optimistic writes and
// background revalidations must not discard the user's unsaved edits;
// `saveSettings` re-seeds explicitly from the server's echo.
let settingsSeeded = false;
watch(
  data,
  (d) => {
    if (!settingsSeeded && d) {
      applySettings(d.settings);
      settingsSeeded = true;
    }
  },
  { immediate: true },
);

async function saveSettings(): Promise<void> {
  busy.value = "settings";
  try {
    // Send every field; blank values reset (remove) that setting.
    const payload: Record<string, string> = { ...settingsForm.value };
    const r = await setPhpSettings(payload);
    applySettings(r.settings);
    toast.success("PHP settings updated", "Pools restart to apply the changes.");
    await reloadPhp();
  } catch (e) {
    toast.error("Couldn't update PHP settings", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function refreshUpdates(): Promise<void> {
  busy.value = "refresh";
  try {
    const r = await checkPhpUpdates();
    mutate((cur) =>
      cur ? { ...cur, installed: r.installed, default: r.default, updates: r.updates ?? [] } : cur,
    );
    toast.success(
      "Update check complete",
      r.updates?.length ? `${r.updates.length} update(s) available` : "All up to date",
    );
  } catch (e) {
    toast.error("Update check failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function makeDefault(v: PhpVersion): Promise<void> {
  busy.value = `default:${v}`;
  try {
    await setDefaultPhp(v);
    mutate((cur) => (cur ? { ...cur, default: v } : cur));
    toast.success(`PHP ${v} is now the default`);
  } catch (e) {
    toast.error("Couldn't set default", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doUpdate(v: PhpVersion | null): Promise<void> {
  busy.value = v ? `update:${v}` : "update:all";
  try {
    await updatePhp(v);
    toast.success(v ? `Updated PHP ${v}` : "Updated all PHP versions");
    // Refresh the status poll too so the new patch shows without the 4s lag.
    await Promise.all([reloadPhp(), refresh()]);
  } catch (e) {
    toast.error("Update failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── process actions ──
// Restart applies to a pool that is up or crashed; an idle/stopped pool has
// nothing to restart (it spawns fresh on the next request).
function canRestart(v: PhpVersion): boolean {
  const s = poolByVersion.value[v]?.state;
  return s === "running" || s === "failed";
}

const anyRunning = computed(() =>
  (report.value?.php ?? []).some((p) => p.state === "running" || p.state === "failed"),
);

async function doRestart(v: PhpVersion): Promise<void> {
  busy.value = `restart:${v}`;
  try {
    await restartPhp(v);
    toast.success(`Restarted PHP ${v}`);
    await refresh();
  } catch (e) {
    toast.error(`Couldn't restart PHP ${v}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doRestartAll(): Promise<void> {
  if (!anyRunning.value) {
    toast.info("No running pools to restart");
    return;
  }
  busy.value = "restart:all";
  try {
    await restartAllPhp();
    toast.success("Restarted all running pools");
    await refresh();
  } catch (e) {
    toast.error("Couldn't restart pools", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── uninstall confirm ──
const uninstallOpen = ref(false);
const uninstallTarget = ref<PhpVersion | null>(null);

// Defer opening past the dropdown's close so reka-ui's focus-restore doesn't
// steal focus from the modal.
function openUninstall(v: PhpVersion): void {
  uninstallTarget.value = v;
  void nextTick(() => {
    uninstallOpen.value = true;
  });
}

async function confirmUninstall(close: () => void): Promise<void> {
  const v = uninstallTarget.value;
  if (!v) return;
  busy.value = `uninstall:${v}`;
  close();
  try {
    await uninstallPhp(v);
    toast.success(`Uninstalled PHP ${v}`);
    await reloadPhp();
  } catch (e) {
    toast.error(`Couldn't uninstall PHP ${v}`, (e as IpcError).message);
  } finally {
    busy.value = null;
    uninstallTarget.value = null;
  }
}

// ── install modal ──
const installOpen = ref(false);
const installLoading = ref(false);
const installOptions = ref<{ value: PhpVersion; label: string }[]>([]);
const selectedVersion = ref<PhpVersion>("");

// Open the modal and fetch the distribution's installable versions, hiding any
// already installed. Pre-selects the LATEST (the daemon returns them ascending,
// so the last entry is newest) so the Select (no placeholder) is always valid.
async function openInstall(): Promise<void> {
  installOpen.value = true;
  installLoading.value = true;
  installOptions.value = [];
  selectedVersion.value = "";
  try {
    const r = await availablePhp();
    const installedSet = new Set(r.installed);
    installOptions.value = r.available
      .filter((v) => !installedSet.has(v))
      .map((v) => ({ value: v, label: `PHP ${v}` }));
    const opts = installOptions.value;
    selectedVersion.value = opts[opts.length - 1]?.value ?? "";
  } catch (e) {
    toast.error("Couldn't load installable versions", (e as IpcError).message);
  } finally {
    installLoading.value = false;
  }
}

async function confirmInstall(close: () => void): Promise<void> {
  const v = selectedVersion.value;
  if (!v) return;
  const opId = `php-install:${v}`;
  operations.begin({ id: opId, kind: "php-install", label: `Installing PHP ${v}` });
  close();
  try {
    await installPhpWithProgress(v, (lines) => {
      const latest = lines[lines.length - 1];
      if (latest) operations.update(opId, { detail: latest });
    });
    toast.success(`Installed PHP ${v}`);
    // Refresh the version list *and* the status poll so the new row shows its
    // patch + "idle" state immediately instead of on the next 4s tick.
    await Promise.all([reloadPhp(), refresh()]);
  } catch (e) {
    toast.error(`Install of PHP ${v} failed`, (e as IpcError).message);
  } finally {
    operations.end(opId);
  }
}

onUnmounted(
  registerViewActions({
    create: () => void openInstall(),
    refresh: () => void reloadPhp(),
  }),
);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="PHP" subtitle="Installed versions, updates, and the global default" />

    <div class="flex-1 overflow-y-auto p-6">
      <!-- Installed versions -->
      <Card>
        <CardHeader class="flex-row items-center justify-between space-y-0">
          <div class="space-y-1.5">
            <CardTitle>Installed versions</CardTitle>
            <CardDescription>Versions, updates, and the global default.</CardDescription>
          </div>
          <div class="flex min-w-0 items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              :disabled="busy === 'refresh'"
              @click="refreshUpdates"
            >
              <Spinner v-if="busy === 'refresh'" class="size-4" />
              <RefreshCw v-else class="size-4" />
              Refresh
            </Button>
            <Button
              variant="outline"
              size="sm"
              :disabled="!hasUpdates || busy === 'update:all'"
              @click="doUpdate(null)"
            >
              <Spinner v-if="busy === 'update:all'" class="size-4" />
              Update all
            </Button>
            <span
              v-if="installDetail"
              class="min-w-0 max-w-[16rem] truncate text-xs text-muted-foreground"
            >
              {{ installDetail }}
            </span>
            <Button size="sm" :disabled="installing" @click="openInstall">
              <Spinner v-if="installing" class="size-4" />
              <Download v-else class="size-4" />
              Install
            </Button>
          </div>
        </CardHeader>

        <CardContent>
          <div v-if="loading" class="flex justify-center py-12"><Spinner class="size-6" /></div>

          <div
            v-else-if="installed.length === 0"
            class="rounded-lg border border-dashed p-10 text-center text-sm text-muted-foreground"
          >
            No PHP versions installed yet. Use <strong>Install</strong> to add one.
          </div>

          <table v-else class="w-full text-sm">
        <thead>
          <tr class="border-b text-left text-xs uppercase text-muted-foreground">
            <th class="py-2 pr-4 font-medium">Version</th>
            <th class="py-2 pr-4 font-medium">FPM</th>
            <th class="py-2 pr-4 font-medium">Patch</th>
            <th class="py-2 pr-4 font-medium">Memory</th>
            <th class="py-2 pr-4 font-medium">Update</th>
            <th class="py-2 pl-4 text-right font-medium">Actions</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="v in installed" :key="v" class="border-b last:border-0">
            <td class="py-3 pr-4">
              <div class="flex items-center gap-2">
                <span class="font-mono font-medium">PHP {{ v }}</span>
                <Badge v-if="v === defaultVersion" variant="secondary">
                  <Star class="size-3" /> default
                </Badge>
              </div>
            </td>
            <td class="py-3 pr-4">
              <StatusPill
                :tone="poolStateTone(poolByVersion[v]?.state)"
                :label="poolStateLabel(poolByVersion[v]?.state)"
              />
            </td>
            <td class="py-3 pr-4 font-mono text-xs text-muted-foreground">
              {{ poolByVersion[v]?.installed_patch ?? "-" }}
            </td>
            <td class="py-3 pr-4 text-xs text-muted-foreground">
              {{ humaniseBytes(poolByVersion[v]?.rss_bytes) }}
            </td>
            <td class="py-3 pr-4">
              <Badge v-if="updateByVersion[v]" variant="warning">
                {{ updateByVersion[v].installed }} → {{ updateByVersion[v].latest }}
              </Badge>
              <span v-else class="text-xs text-muted-foreground">up to date</span>
            </td>
            <td class="py-3 pl-4">
              <div class="flex items-center justify-end">
                <Spinner v-if="busy?.endsWith(`:${v}`)" class="size-4" />
                <DropdownMenu>
                  <DropdownMenuTrigger as-child>
                    <Button variant="ghost" size="icon" :aria-label="`Actions for PHP ${v}`">
                      <MoreHorizontal class="size-4" />
                    </Button>
                  </DropdownMenuTrigger>
                  <DropdownMenuContent align="end">
                    <DropdownMenuItem :disabled="!canRestart(v)" @select="doRestart(v)">
                      <RotateCw class="size-4" /> Restart
                    </DropdownMenuItem>
                    <DropdownMenuItem :disabled="!updateByVersion[v]" @select="doUpdate(v)">
                      <Download class="size-4" /> Update
                    </DropdownMenuItem>
                    <DropdownMenuItem :disabled="v === defaultVersion" @select="makeDefault(v)">
                      <Star class="size-4" /> Set default
                    </DropdownMenuItem>
                    <DropdownMenuSeparator />
                    <DropdownMenuItem
                      class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                      @select="openUninstall(v)"
                    >
                      <Trash2 class="size-4" /> Uninstall
                    </DropdownMenuItem>
                  </DropdownMenuContent>
                </DropdownMenu>
              </div>
            </td>
          </tr>
            </tbody>
          </table>

          <div class="mt-4 flex items-center justify-between gap-2">
          <span class="text-xs text-muted-foreground">
            Updates are notify-only; nothing installs without your action.
          </span>
          <Button
            variant="outline"
            size="sm"
            :disabled="!anyRunning || busy === 'restart:all'"
            @click="doRestartAll"
          >
            <Spinner v-if="busy === 'restart:all'" class="size-4" />
            <RotateCw v-else class="size-4" />
            Restart all
          </Button>
          </div>
        </CardContent>
      </Card>

      <!-- Global PHP ini defaults, applied to every installed version. -->
      <Card v-if="!loading" class="mt-8">
        <CardHeader>
          <CardTitle>Default settings</CardTitle>
          <CardDescription>
            Applied to every installed PHP version. Leave a field blank to use
            PHP's built-in default. Saving restarts the running pools.
          </CardDescription>
        </CardHeader>

        <CardContent>
          <TooltipProvider :delay-duration="0">
          <div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
            <div v-for="s in TEXT_SETTINGS" :key="s.key">
              <div class="flex items-center gap-1">
                <label class="text-xs font-medium" :for="`set-${s.key}`">{{ s.label }}</label>
                <Tooltip>
                  <TooltipTrigger as-child>
                    <span class="inline-flex cursor-help text-muted-foreground">
                      <Info class="size-3.5" />
                    </span>
                  </TooltipTrigger>
                  <TooltipContent side="top">{{ s.hint }}</TooltipContent>
                </Tooltip>
              </div>
              <Input
                :id="`set-${s.key}`"
                v-model="settingsForm[s.key]"
                :placeholder="s.placeholder"
                class="mt-1"
              />
            </div>
            <div>
              <div class="flex items-center gap-1">
                <span class="text-xs font-medium">Display errors</span>
                <Tooltip>
                  <TooltipTrigger as-child>
                    <span class="inline-flex cursor-help text-muted-foreground">
                      <Info class="size-3.5" />
                    </span>
                  </TooltipTrigger>
                  <TooltipContent side="top">{{ DISPLAY_ERRORS_HINT }}</TooltipContent>
                </Tooltip>
              </div>
              <div class="mt-1">
                <Select
                  class="w-full"
                  :model-value="settingsForm.display_errors ?? ''"
                  :options="DISPLAY_ERRORS_OPTIONS"
                  aria-label="display_errors"
                  @update:model-value="(v: string) => (settingsForm.display_errors = v)"
                />
              </div>
            </div>
          </div>
          </TooltipProvider>

          <div class="mt-5 flex justify-end">
            <Button size="sm" :disabled="busy === 'settings'" @click="saveSettings">
              <Spinner v-if="busy === 'settings'" class="size-4" />
              {{ busy === "settings" ? "Applying…" : "Save" }}
            </Button>
          </div>
        </CardContent>
      </Card>
    </div>

    <Modal v-model:open="installOpen" title="Install a PHP version">
      <div v-if="installLoading" class="flex justify-center py-6">
        <Spinner class="size-5" />
      </div>
      <template v-else-if="installOptions.length">
        <span class="text-sm font-medium">Version</span>
        <div class="mt-2">
          <Select
            class="w-full"
            :model-value="selectedVersion"
            :options="installOptions"
            aria-label="PHP version to install"
            @update:model-value="(v: PhpVersion) => (selectedVersion = v)"
          />
        </div>
        <p class="mt-2 text-xs text-muted-foreground">
          Downloads a prebuilt static build; this can take a few minutes with no
          progress bar (the daemon reports only on completion).
        </p>
      </template>
      <p v-else class="py-2 text-sm text-muted-foreground">
        No installable versions to add - every version offered by the
        distribution is already installed, or it couldn't be reached.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button
          :disabled="!installOptions.length || !selectedVersion"
          @click="confirmInstall(close)"
        >
          Install
        </Button>
      </template>
    </Modal>

    <Modal v-model:open="uninstallOpen" title="Uninstall PHP version">
      <p class="text-sm text-muted-foreground">
        Remove <strong class="font-mono text-foreground">PHP {{ uninstallTarget }}</strong>
        and its files? This stops its pool. Sites using it, or removing your last
        version, will be blocked.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button variant="destructive" @click="confirmUninstall(close)">Uninstall</Button>
      </template>
    </Modal>
  </div>
</template>
