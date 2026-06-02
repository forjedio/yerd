<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import {
  ExternalLink,
  FolderMinus,
  FolderOpen,
  FolderPlus,
  FolderTree,
  Globe,
  Info,
  Link2,
  Lock,
  LockOpen,
  MoreHorizontal,
  Pencil,
  Search,
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
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
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
  setWebRoot,
  unlink,
  unpark,
} from "@/ipc/client";
import type { Site } from "@/ipc/types";

const toast = useToast();
const { report } = useDaemon();

const sites = ref<Site[]>([]);
const parked = ref<string[]>([]);
const loading = ref(true);
const rowBusy = ref<string | null>(null);
const siteFilter = ref("");

const tld = computed(() => report.value?.tld ?? "test");
const caTrusted = computed(() => report.value?.ca.trusted_system === true);

// PHP options for the edit form, from the live status report.
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

const folderRows = computed(() =>
  parked.value.map((folder) => ({
    folder,
    count: sites.value.filter(
      (s) => s.kind === "parked" && parentDir(s.document_root) === folder,
    ).length,
  })),
);

// Live, case-insensitive filter on the full `<name>.<tld>` domain.
const filteredSites = computed(() => {
  const q = siteFilter.value.trim().toLowerCase();
  if (!q) return sites.value;
  return sites.value.filter((s) =>
    `${s.name}.${tld.value}`.toLowerCase().includes(q),
  );
});

function siteUrl(s: Site): string {
  const scheme = s.secure ? "https" : "http";
  const bound = s.secure ? report.value?.https.bound : report.value?.http.bound;
  const dflt = s.secure ? 443 : 80;
  const redirected = report.value?.port_redirect === true;
  const port = !redirected && bound && bound !== dflt ? `:${bound}` : "";
  return `${scheme}://${s.name}.${tld.value}${port}`;
}

/** The served sub-directory label for a site ("/" when the project root is served). */
function servedLabel(s: Site): string {
  return s.web_subpath && s.web_subpath !== "" ? s.web_subpath : "/";
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

// ── edit site (PHP + web root + HTTPS) ──
const editOpen = ref(false);
const editTarget = ref<Site | null>(null);
const editPhp = ref<string>("");
const editWebRoot = ref("");
const editSecure = ref(false);

function openEdit(s: Site): void {
  editTarget.value = s;
  editPhp.value = s.php;
  editWebRoot.value = s.web_subpath ?? "";
  editSecure.value = s.secure;
  // Defer past the dropdown's close so reka-ui's focus-restore doesn't steal
  // focus from the modal.
  void nextTick(() => {
    editOpen.value = true;
  });
}

async function chooseEditDir(): Promise<void> {
  // The picker only suggests a start dir; the daemon enforces containment.
  const dir = await pickDirectory(editTarget.value?.document_root);
  if (dir) editWebRoot.value = dir;
}

async function confirmEdit(close: () => void): Promise<void> {
  const s = editTarget.value;
  close();
  if (!s) return;
  rowBusy.value = `edit:${s.name}`;
  try {
    // Apply only what changed; each setter restarts/re-renders as needed.
    if (editPhp.value && editPhp.value !== s.php) {
      await setPhp(s.name, editPhp.value);
    }
    const newRoot = editWebRoot.value.trim();
    if (newRoot !== (s.web_subpath ?? "")) {
      await setWebRoot(s.name, newRoot === "" ? null : newRoot);
    }
    if (editSecure.value !== s.secure) {
      await setSecure(s.name, editSecure.value);
    }
    toast.success(`Updated ${s.name}`);
    await load();
  } catch (e) {
    toast.error("Couldn't update site", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
    editTarget.value = null;
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
          <RouterLink to="/general" class="font-medium underline">General → Environment</RouterLink>.
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

            <template v-else>
              <div class="relative mb-4 max-w-xs">
                <Search
                  class="pointer-events-none absolute left-2.5 top-1/2 size-4 -translate-y-1/2 text-muted-foreground"
                />
                <Input
                  v-model="siteFilter"
                  placeholder="Filter by domain…"
                  aria-label="Filter sites by domain"
                  class="pl-8"
                />
              </div>

              <TooltipProvider :delay-duration="0">
              <table class="w-full text-sm">
                <thead>
                  <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                    <th class="py-2 pr-4 font-medium">Site</th>
                    <th class="py-2 pr-4 font-medium">PHP</th>
                    <th class="py-2 pr-4 font-medium">HTTPS</th>
                    <th class="py-2 pl-4 text-right font-medium">Actions</th>
                  </tr>
                </thead>
                <tbody>
                  <tr v-for="s in filteredSites" :key="s.name" class="border-b last:border-0">
                    <td class="py-4 pr-4">
                      <div class="flex items-center gap-2">
                        <button
                          class="font-medium text-primary hover:underline"
                          @click="openInBrowser(siteUrl(s))"
                        >
                          {{ s.name }}.{{ tld }}
                        </button>
                        <Badge variant="outline">{{ s.kind }}</Badge>
                        <Tooltip>
                          <TooltipTrigger as-child>
                            <span class="inline-flex cursor-help text-muted-foreground">
                              <Info class="size-3.5" />
                            </span>
                          </TooltipTrigger>
                          <TooltipContent side="top">
                            <div class="space-y-0.5 text-xs">
                              <div>
                                <span class="text-muted-foreground">Document root:</span>
                                {{ s.document_root }}
                              </div>
                              <div>
                                <span class="text-muted-foreground">Served from:</span>
                                {{ servedLabel(s) }}
                              </div>
                            </div>
                          </TooltipContent>
                        </Tooltip>
                      </div>
                    </td>
                    <td class="py-4 pr-4">
                      <span class="font-mono text-xs">PHP {{ s.php }}</span>
                    </td>
                    <td class="py-4 pr-4">
                      <Tooltip>
                        <TooltipTrigger as-child>
                          <span class="inline-flex cursor-help">
                            <Lock v-if="s.secure" class="size-4 text-success" />
                            <LockOpen v-else class="size-4 text-muted-foreground" />
                          </span>
                        </TooltipTrigger>
                        <TooltipContent side="top">{{ s.secure ? "HTTPS enabled" : "HTTP only" }}</TooltipContent>
                      </Tooltip>
                    </td>
                    <td class="py-4 pl-4">
                      <div class="flex items-center justify-end">
                        <Spinner v-if="rowBusy?.endsWith(`:${s.name}`)" class="size-4" />
                        <DropdownMenu>
                          <DropdownMenuTrigger as-child>
                            <Button variant="ghost" size="icon" :aria-label="`Actions for ${s.name}`">
                              <MoreHorizontal class="size-4" />
                            </Button>
                          </DropdownMenuTrigger>
                          <DropdownMenuContent align="end">
                            <DropdownMenuItem @select="openEdit(s)">
                              <Pencil class="size-4" /> Edit…
                            </DropdownMenuItem>
                            <DropdownMenuItem @select="openInBrowser(siteUrl(s))">
                              <ExternalLink class="size-4" /> Open in browser
                            </DropdownMenuItem>
                            <DropdownMenuItem @select="openPath(s.document_root)">
                              <FolderOpen class="size-4" /> Reveal folder
                            </DropdownMenuItem>
                            <!-- Only linked sites are removable here (by name).
                                 A parked site is removed by un-parking its folder. -->
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
              </TooltipProvider>

              <p
                v-if="filteredSites.length === 0"
                class="py-8 text-center text-sm text-muted-foreground"
              >
                No sites match “{{ siteFilter }}”.
              </p>
            </template>
          </CardContent>
        </Card>
      </template>
    </div>

    <!-- edit site modal -->
    <Modal v-model:open="editOpen" :title="`Edit ${editTarget?.name ?? ''}`">
      <div class="space-y-4">
        <div>
          <label for="edit-php-version" class="text-sm font-medium">PHP version</label>
          <div class="mt-2">
            <Select
              v-if="phpOptions"
              id="edit-php-version"
              :model-value="editPhp"
              :options="phpOptions"
              class="w-full"
              aria-label="PHP version"
              @update:model-value="(v: string) => (editPhp = v)"
            />
            <p v-else class="text-xs text-muted-foreground">No PHP versions installed.</p>
          </div>
        </div>

        <div>
          <label class="block text-sm font-medium" for="editwebroot">Web root</label>
          <div class="mt-2 flex gap-2">
            <Input id="editwebroot" v-model="editWebRoot" placeholder="public" />
            <Button variant="outline" @click="chooseEditDir"><FolderOpen class="size-4" /></Button>
          </div>
          <p class="mt-1 text-xs text-muted-foreground">
            Directory served as the document root, relative to the site folder
            (e.g. <code class="font-mono">public</code>). Leave blank to auto-detect.
          </p>
        </div>

        <div class="flex items-center justify-between gap-4">
          <div>
            <p class="text-sm font-medium">HTTPS</p>
            <p class="text-xs text-muted-foreground">Serve this site over TLS.</p>
          </div>
          <Switch v-model="editSecure" aria-label="HTTPS" />
        </div>
      </div>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmEdit(close)">Save</Button>
      </template>
    </Modal>

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
