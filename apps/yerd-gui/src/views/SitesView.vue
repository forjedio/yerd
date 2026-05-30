<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import {
  ExternalLink,
  FolderMinus,
  FolderOpen,
  FolderPlus,
  FolderTree,
  Globe,
  Link2,
  MoreHorizontal,
  ShieldAlert,
  Trash2,
} from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
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
import Switch from "@/components/ui/Switch.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  IpcError,
  link,
  listParked,
  listSites,
  openInBrowser,
  openPath,
  park,
  pickDirectory,
  setPhp,
  setSecure,
  unlink,
  unpark,
} from "@/ipc/client";
import type { PhpVersion, Site } from "@/ipc/types";

const toast = useToast();
const { report } = useDaemon();

const sites = ref<Site[]>([]);
const parked = ref<string[]>([]);
const loading = ref(true);
const rowBusy = ref<string | null>(null);

const tld = computed(() => report.value?.tld ?? "test");
const caTrusted = computed(() => report.value?.ca.trusted_system === true);

// PHP options for the per-site picker, from the live status report.
const phpOptions = computed(() => {
  const versions = (report.value?.php ?? []).map((p) => p.version);
  const opts = versions.map((v) => ({ value: v, label: `PHP ${v}` }));
  return opts.length ? opts : null;
});

/** The parent directory of a path (slice before the last `/` or `\`). */
function parentDir(p: string): string {
  const i = Math.max(p.lastIndexOf("/"), p.lastIndexOf("\\"));
  return i <= 0 ? p : p.slice(0, i);
}

// One row per registered parked root, with a count of the parked sites it
// produces. Strict parent-equality (not prefix) so a nested root isn't double
// counted; only `kind === "parked"` sites belong to a folder.
const folderRows = computed(() =>
  parked.value.map((folder) => ({
    folder,
    count: sites.value.filter(
      (s) => s.kind === "parked" && parentDir(s.document_root) === folder,
    ).length,
  })),
);

function siteUrl(s: Site): string {
  const scheme = s.secure ? "https" : "http";
  const bound = s.secure ? report.value?.https.bound : report.value?.http.bound;
  const dflt = s.secure ? 443 : 80;
  const port = bound && bound !== dflt ? `:${bound}` : "";
  return `${scheme}://${s.name}.${tld.value}${port}`;
}

async function load(): Promise<void> {
  loading.value = true;
  try {
    const [s, p] = await Promise.all([listSites(), listParked()]);
    sites.value = s;
    parked.value = p;
  } catch (e) {
    toast.error("Couldn't load sites", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

async function onSetPhp(s: Site, version: PhpVersion): Promise<void> {
  rowBusy.value = `php:${s.name}`;
  try {
    await setPhp(s.name, version);
    toast.success(`${s.name} now uses PHP ${version}`);
    await load();
  } catch (e) {
    toast.error("Couldn't change PHP version", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function onToggleSecure(s: Site, secure: boolean): Promise<void> {
  rowBusy.value = `secure:${s.name}`;
  try {
    await setSecure(s.name, secure);
    toast.success(secure ? `Enabled HTTPS for ${s.name}` : `Disabled HTTPS for ${s.name}`);
    await load();
  } catch (e) {
    toast.error("Couldn't change HTTPS", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function onPark(): Promise<void> {
  const dir = await pickDirectory();
  if (!dir) return;
  rowBusy.value = "park";
  try {
    await park(dir);
    toast.success("Parked directory", dir);
    await load();
  } catch (e) {
    toast.error("Park failed", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

// ── un-park confirm ──
const unparkOpen = ref(false);
const unparkTarget = ref<string | null>(null);

// Defer opening past the dropdown's close so reka-ui's focus-restore doesn't
// steal focus from the modal (same pattern as PhpView's uninstall).
function openUnpark(folder: string): void {
  unparkTarget.value = folder;
  void nextTick(() => {
    unparkOpen.value = true;
  });
}

async function confirmUnpark(close: () => void): Promise<void> {
  const folder = unparkTarget.value;
  if (!folder) return;
  close();
  rowBusy.value = `unpark:${folder}`;
  try {
    await unpark(folder);
    toast.success("Un-parked folder", folder);
    await load();
  } catch (e) {
    toast.error("Un-park failed", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
    unparkTarget.value = null;
  }
}

// ── link modal ──
const linkOpen = ref(false);
const linkName = ref("");
const linkPath = ref("");
const linkValid = computed(
  () => /^[a-z0-9-]+$/i.test(linkName.value.trim()) && linkPath.value.trim() !== "",
);

async function chooseLinkDir(): Promise<void> {
  const dir = await pickDirectory();
  if (dir) linkPath.value = dir;
}

async function confirmLink(close: () => void): Promise<void> {
  const name = linkName.value.trim();
  const path = linkPath.value.trim();
  close();
  rowBusy.value = "link";
  try {
    await link(name, path);
    toast.success(`Linked ${name}`);
    linkName.value = "";
    linkPath.value = "";
    await load();
  } catch (e) {
    toast.error("Link failed", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

// ── remove (unlink) confirm ──
const unlinkOpen = ref(false);
const unlinkTarget = ref<Site | null>(null);

function openUnlink(s: Site): void {
  unlinkTarget.value = s;
  void nextTick(() => {
    unlinkOpen.value = true;
  });
}

async function confirmUnlink(close: () => void): Promise<void> {
  const s = unlinkTarget.value;
  close();
  if (!s) return;
  rowBusy.value = `unlink:${s.name}`;
  try {
    await unlink(s.name);
    toast.success(`Removed ${s.name}`);
    await load();
  } catch (e) {
    toast.error("Couldn't remove site", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
    unlinkTarget.value = null;
  }
}

onMounted(load);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Sites" subtitle="Parked and linked .test sites" />

    <div class="flex-1 overflow-y-auto p-6">
      <div
        v-if="!caTrusted && report"
        class="mb-4 flex items-start gap-2 rounded-md border border-warning/40 bg-warning/10 p-3 text-xs"
      >
        <ShieldAlert class="mt-0.5 size-4 shrink-0 text-warning" />
        <span>
          The local CA isn't trusted in your system store, so browsers will warn
          on HTTPS sites. Fix it under
          <RouterLink to="/services" class="font-medium underline">Services → Environment</RouterLink>.
        </span>
      </div>

      <div v-if="loading" class="flex justify-center py-16"><Spinner class="size-6" /></div>

      <template v-else>
        <!-- Parked folders -->
        <Card>
          <CardHeader class="flex-row items-center justify-between space-y-0">
            <div class="space-y-1.5">
              <CardTitle class="flex items-center gap-2"><FolderTree class="size-4" /> Parked folders</CardTitle>
              <CardDescription>
                Each child directory of a parked folder is served as a .test site.
              </CardDescription>
            </div>
            <Button variant="outline" size="sm" :disabled="rowBusy === 'park'" @click="onPark">
              <Spinner v-if="rowBusy === 'park'" class="size-4" />
              <FolderPlus v-else class="size-4" />
              Park folder
            </Button>
          </CardHeader>

          <CardContent>
            <div
              v-if="folderRows.length === 0"
              class="rounded-lg border border-dashed p-10 text-center text-sm text-muted-foreground"
            >
              No parked folders yet. <strong>Park</strong> a folder of projects to
              serve each child directory automatically.
            </div>

            <table v-else class="w-full text-sm">
              <thead>
                <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                  <th class="py-2 pr-4 font-medium">Folder</th>
                  <th class="py-2 pr-4 font-medium">Sites</th>
                  <th class="py-2 pl-4 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="row in folderRows" :key="row.folder" class="border-b last:border-0">
                  <td class="py-3 pr-4">
                    <button
                      class="flex items-center gap-1.5 truncate text-xs text-muted-foreground hover:text-foreground"
                      :title="row.folder"
                      @click="openPath(row.folder)"
                    >
                      <FolderOpen class="size-3.5 shrink-0" />
                      <span class="truncate">{{ row.folder }}</span>
                    </button>
                  </td>
                  <td class="py-3 pr-4">
                    <Badge variant="secondary">{{ row.count }}</Badge>
                  </td>
                  <td class="py-3 pl-4">
                    <div class="flex items-center justify-end">
                      <Spinner v-if="rowBusy === `unpark:${row.folder}`" class="size-4" />
                      <DropdownMenu>
                        <DropdownMenuTrigger as-child>
                          <Button
                            variant="ghost"
                            size="icon"
                            :aria-label="`Actions for ${row.folder}`"
                          >
                            <MoreHorizontal class="size-4" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem @select="openPath(row.folder)">
                            <FolderOpen class="size-4" /> Reveal folder
                          </DropdownMenuItem>
                          <DropdownMenuSeparator />
                          <DropdownMenuItem
                            class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                            @select="openUnpark(row.folder)"
                          >
                            <FolderMinus class="size-4" /> Un-park
                          </DropdownMenuItem>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </td>
                </tr>
              </tbody>
            </table>
          </CardContent>
        </Card>

        <!-- Sites -->
        <Card class="mt-8">
          <CardHeader class="flex-row items-center justify-between space-y-0">
            <div class="space-y-1.5">
              <CardTitle class="flex items-center gap-2"><Globe class="size-4" /> Sites</CardTitle>
              <CardDescription>Every parked and linked .test site.</CardDescription>
            </div>
            <Button size="sm" @click="linkOpen = true">
              <Link2 class="size-4" /> Link site
            </Button>
          </CardHeader>

          <CardContent>
            <div
              v-if="sites.length === 0"
              class="rounded-lg border border-dashed p-10 text-center text-sm text-muted-foreground"
            >
              No sites yet. <strong>Park</strong> a folder of projects or
              <strong>Link</strong> a single directory.
            </div>

            <table v-else class="w-full text-sm">
              <thead>
                <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                  <th class="py-2 pr-4 font-medium">Site</th>
                  <th class="py-2 pr-4 font-medium">Document root</th>
                  <th class="py-2 pr-4 font-medium">PHP</th>
                  <th class="py-2 pr-4 font-medium">HTTPS</th>
                  <th class="py-2 pl-4 text-right font-medium">Actions</th>
                </tr>
              </thead>
              <tbody>
                <tr v-for="s in sites" :key="s.name" class="border-b last:border-0">
                  <td class="py-3 pr-4">
                    <div class="flex items-center gap-2">
                      <button
                        class="font-medium text-primary hover:underline"
                        @click="openInBrowser(siteUrl(s))"
                      >
                        {{ s.name }}.{{ tld }}
                      </button>
                      <Badge variant="outline">{{ s.kind }}</Badge>
                    </div>
                  </td>
                  <td class="py-3 pr-4">
                    <button
                      class="flex items-center gap-1.5 truncate text-xs text-muted-foreground hover:text-foreground"
                      :title="s.document_root"
                      @click="openPath(s.document_root)"
                    >
                      <FolderOpen class="size-3.5 shrink-0" />
                      <span class="truncate">{{ s.document_root }}</span>
                    </button>
                  </td>
                  <td class="py-3 pr-4">
                    <Select
                      v-if="phpOptions"
                      :model-value="s.php"
                      :options="phpOptions"
                      :disabled="rowBusy === `php:${s.name}`"
                      :aria-label="`PHP version for ${s.name}`"
                      @update:model-value="(v: string) => onSetPhp(s, v)"
                    />
                    <span v-else class="font-mono text-xs">PHP {{ s.php }}</span>
                  </td>
                  <td class="py-3 pr-4">
                    <Switch
                      :model-value="s.secure"
                      :disabled="rowBusy === `secure:${s.name}`"
                      :aria-label="`HTTPS for ${s.name}`"
                      @update:model-value="(v: boolean) => onToggleSecure(s, v)"
                    />
                  </td>
                  <td class="py-3 pl-4">
                    <div class="flex items-center justify-end">
                      <Spinner v-if="rowBusy === `unlink:${s.name}`" class="size-4" />
                      <DropdownMenu>
                        <DropdownMenuTrigger as-child>
                          <Button variant="ghost" size="icon" :aria-label="`Actions for ${s.name}`">
                            <MoreHorizontal class="size-4" />
                          </Button>
                        </DropdownMenuTrigger>
                        <DropdownMenuContent align="end">
                          <DropdownMenuItem @select="openInBrowser(siteUrl(s))">
                            <ExternalLink class="size-4" /> Open in browser
                          </DropdownMenuItem>
                          <DropdownMenuItem @select="openPath(s.document_root)">
                            <FolderOpen class="size-4" /> Reveal folder
                          </DropdownMenuItem>
                          <!-- Only linked sites are removable here (by name).
                               A parked site has no destructive action — it's
                               removed by un-parking its folder. -->
                          <template v-if="s.kind === 'linked'">
                            <DropdownMenuSeparator />
                            <DropdownMenuItem
                              class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                              @select="openUnlink(s)"
                            >
                              <Trash2 class="size-4" /> Unlink
                            </DropdownMenuItem>
                          </template>
                        </DropdownMenuContent>
                      </DropdownMenu>
                    </div>
                  </td>
                </tr>
              </tbody>
            </table>
          </CardContent>
        </Card>
      </template>
    </div>

    <!-- link modal -->
    <Modal v-model:open="linkOpen" title="Link a site">
      <label class="text-sm font-medium" for="linkname">Name (single label)</label>
      <Input id="linkname" v-model="linkName" placeholder="e.g. myapp" class="mt-2" />
      <label class="mt-4 block text-sm font-medium" for="linkpath">Directory</label>
      <div class="mt-2 flex gap-2">
        <Input id="linkpath" v-model="linkPath" placeholder="/path/to/project" />
        <Button variant="outline" @click="chooseLinkDir"><FolderOpen class="size-4" /></Button>
      </div>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button :disabled="!linkValid" @click="confirmLink(close)">Link</Button>
      </template>
    </Modal>

    <!-- un-park confirm -->
    <Modal
      v-model:open="unparkOpen"
      title="Un-park folder"
      @update:open="(v: boolean) => { if (!v) unparkTarget = null; }"
    >
      <p class="text-sm text-muted-foreground">
        Un-park <strong class="font-mono text-foreground">{{ unparkTarget }}</strong>?
        Its child directories stop being served as .test sites. Linked sites are
        untouched.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button variant="destructive" @click="confirmUnpark(close)">Un-park</Button>
      </template>
    </Modal>

    <!-- remove (unlink) confirm -->
    <Modal
      v-model:open="unlinkOpen"
      title="Remove site"
      @update:open="(v: boolean) => { if (!v) unlinkTarget = null; }"
    >
      <p class="text-sm text-muted-foreground">
        Remove <strong>{{ unlinkTarget?.name }}.{{ tld }}</strong>? Parked sites
        re-appear if their folder is still under a parked directory.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button variant="destructive" @click="confirmUnlink(close)">Remove</Button>
      </template>
    </Modal>
  </div>
</template>
