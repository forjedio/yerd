<script setup lang="ts">
import { computed, nextTick, onMounted, ref } from "vue";
import {
  ChevronDown,
  ExternalLink,
  FolderMinus,
  FolderOpen,
  FolderPlus,
  FolderTree,
  Globe,
  Link2,
  Lock,
  LockOpen,
  MoreHorizontal,
  Package,
  Pencil,
  Plus,
  Rocket,
  Search,
  ShieldAlert,
  Trash2,
} from "lucide-vue-next";

import CreateLaravelWizard from "@/components/site-create/CreateLaravelWizard.vue";
import PageHeader from "@/components/PageHeader.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import AsyncState from "@/components/ui/AsyncState.vue";
import EmptyState from "@/components/ui/EmptyState.vue";
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
import { openTitle, siteUrl } from "@/lib/siteUrl";
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
const error = ref<string | null>(null);
const rowBusy = ref<string | null>(null);
const siteFilter = ref("");

const tld = computed(() => report.value?.tld ?? "test");
const caTrusted = computed(() => report.value?.ca.trusted_system === true);

// ── create new site ──
const createOpen = ref(false);
const phpVersionList = computed(() => (report.value?.php ?? []).map((p) => p.version));
const defaultPhp = computed(() => report.value?.default_php ?? "");

function openCreate(): void {
  // Defer past the dropdown's close so reka-ui's focus-restore doesn't fight the modal.
  void nextTick(() => {
    createOpen.value = true;
  });
}

async function onCreated(): Promise<void> {
  await load();
}

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

// Live, case-insensitive filter on the full `<name>.<tld>` domain, secured
// first then alphabetical so the list is stable and scannable.
const filteredSites = computed(() => {
  const q = siteFilter.value.trim().toLowerCase();
  const list = q
    ? sites.value.filter((s) => `${s.name}.${tld.value}`.toLowerCase().includes(q))
    : [...sites.value];
  return list.sort(
    (a, b) => Number(b.secure) - Number(a.secure) || a.name.localeCompare(b.name),
  );
});


/** The served sub-directory label for a site ("/" when the project root is served). */
function servedLabel(s: Site): string {
  return s.web_subpath && s.web_subpath !== "" ? s.web_subpath : "/";
}

async function load(): Promise<void> {
  loading.value = true;
  error.value = null;
  try {
    const [s, p] = await Promise.all([listSites(), listParked()]);
    sites.value = s;
    parked.value = p;
  } catch (e) {
    error.value = (e as IpcError).message;
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
    <PageHeader title="Sites" subtitle="Parked and linked .test sites">
      <template #actions>
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button size="sm">
              <Plus class="size-4" /> Create <ChevronDown class="size-3.5 opacity-70" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-56">
            <DropdownMenuItem @select="openCreate">
              <Rocket class="size-4" /> New Laravel site…
            </DropdownMenuItem>
            <!-- Future frameworks slot in here. -->
            <DropdownMenuItem
              disabled
              class="opacity-60"
              @select.prevent
            >
              <Package class="size-4" /> Other frameworks
              <span class="ml-auto rounded bg-muted px-1.5 py-0.5 text-[10px] font-medium">Soon</span>
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem @select="linkOpen = true">
              <Link2 class="size-4" /> Link existing site
            </DropdownMenuItem>
            <DropdownMenuItem @select="onPark">
              <FolderPlus class="size-4" /> Park folder
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </template>
    </PageHeader>

    <div class="flex-1 overflow-y-auto p-6">
      <!-- CA-not-trusted warning (sites still serve, browsers just warn). -->
      <div
        v-if="!caTrusted && report"
        class="mb-4 flex items-start gap-2 rounded-md border border-warning/40 bg-warning/10 p-3 text-xs"
      >
        <ShieldAlert class="mt-0.5 size-4 shrink-0 text-warning" />
        <span>
          The local CA isn't trusted in your system store, so browsers will warn
          on HTTPS sites. Fix it under
          <RouterLink to="/doctor" class="font-medium underline">Doctor → Environment</RouterLink>.
        </span>
      </div>

      <AsyncState
        :loading="loading"
        :error="error"
        :empty="sites.length === 0 && parked.length === 0"
        pad="py-16"
        @retry="load"
      >
        <template #empty>
          <EmptyState
            :icon="Globe"
            title="No sites yet"
            description="Park a folder of projects to serve every child directory, or link a single project directory."
          >
            <div class="flex gap-2">
              <Button @click="openCreate"><Rocket class="size-4" /> New Laravel site</Button>
              <Button variant="outline" :disabled="rowBusy === 'park'" @click="onPark">
                <FolderPlus class="size-4" /> Park folder
              </Button>
              <Button variant="outline" @click="linkOpen = true"><Link2 class="size-4" /> Link site</Button>
            </div>
          </EmptyState>
        </template>

        <!-- Toolbar: filter + count -->
        <div
          v-if="sites.length"
          class="mb-4 flex items-center justify-between gap-3"
        >
          <div class="relative max-w-xs flex-1">
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
          <span class="shrink-0 text-xs text-muted-foreground">
            {{ filteredSites.length }} of {{ sites.length }}
          </span>
        </div>

        <!-- Site grid -->
        <div
          v-if="filteredSites.length"
          class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3"
        >
          <div
            v-for="s in filteredSites"
            :key="s.name"
            class="group rounded-lg border bg-card p-4 shadow-sm transition-colors hover:border-brand/40"
          >
            <div class="flex items-start justify-between gap-2">
              <div class="min-w-0">
                <button
                  class="flex max-w-full items-center gap-1.5 font-mono text-sm font-medium hover:text-brand"
                  :title="openTitle(s, report)"
                  @click="openInBrowser(siteUrl(s, report))"
                >
                  <span class="truncate">{{ s.name }}.{{ tld }}</span>
                </button>
                <button
                  class="mt-1 flex max-w-full items-center gap-1 text-xs text-muted-foreground hover:text-foreground"
                  :title="`Reveal ${s.document_root}`"
                  @click="openPath(s.document_root)"
                >
                  <FolderOpen class="size-3 shrink-0" />
                  <span class="truncate font-mono">{{ s.document_root }}</span>
                </button>
              </div>

              <div class="flex shrink-0 items-center">
                <Spinner v-if="rowBusy?.endsWith(`:${s.name}`)" class="size-4" />
                <Button
                  variant="ghost"
                  size="icon"
                  :aria-label="openTitle(s, report)"
                  :title="openTitle(s, report)"
                  @click="openInBrowser(siteUrl(s, report))"
                >
                  <ExternalLink class="size-4" />
                </Button>
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
                    <DropdownMenuItem @select="openInBrowser(siteUrl(s, report))">
                      <ExternalLink class="size-4" /> Open in browser
                    </DropdownMenuItem>
                    <DropdownMenuItem @select="openPath(s.document_root)">
                      <FolderOpen class="size-4" /> Reveal folder
                    </DropdownMenuItem>
                    <!-- Only linked sites are removable here (by name). A parked
                         site is removed by un-parking its folder. -->
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
            </div>

            <!-- meta chips -->
            <div class="mt-3 flex flex-wrap items-center gap-1.5">
              <span
                class="inline-flex items-center rounded-md bg-muted px-1.5 py-0.5 font-mono text-[11px] font-medium text-muted-foreground"
              >
                PHP {{ s.php }}
              </span>
              <span
                v-if="s.secure"
                class="inline-flex items-center gap-1 rounded-md bg-success/10 px-1.5 py-0.5 text-[11px] font-medium text-success"
              >
                <Lock class="size-3" /> HTTPS
              </span>
              <span
                v-else
                class="inline-flex items-center gap-1 rounded-md bg-muted px-1.5 py-0.5 text-[11px] font-medium text-muted-foreground"
              >
                <LockOpen class="size-3" /> HTTP
              </span>
              <span
                v-if="s.web_subpath"
                class="inline-flex items-center rounded-md bg-muted px-1.5 py-0.5 font-mono text-[11px] text-muted-foreground"
                :title="`Serves ${servedLabel(s)} as the document root`"
              >
                /{{ servedLabel(s) }}
              </span>
              <span
                class="ml-auto inline-flex items-center gap-1 text-[11px] text-muted-foreground"
              >
                <Link2 v-if="s.kind === 'linked'" class="size-3" />
                <FolderTree v-else class="size-3" />
                {{ s.kind }}
              </span>
            </div>
          </div>
        </div>

        <!-- filtered to nothing -->
        <p
          v-else-if="siteFilter"
          class="py-12 text-center text-sm text-muted-foreground"
        >
          No sites match “{{ siteFilter }}”.
        </p>
        <p v-else class="py-12 text-center text-sm text-muted-foreground">
          Your parked folders have no child directories yet.
        </p>

        <!-- Parked folders (demoted: the management surface, below the sites) -->
        <section v-if="folderRows.length" class="mt-8">
          <div class="mb-2">
            <h3 class="text-sm font-semibold">Parked folders</h3>
            <p class="text-xs text-muted-foreground">
              Each child directory of a parked folder is served automatically.
            </p>
          </div>
          <div class="divide-y rounded-lg border">
            <div
              v-for="row in folderRows"
              :key="row.folder"
              class="flex items-center justify-between gap-3 px-3 py-2.5"
            >
              <button
                class="flex min-w-0 items-center gap-1.5 text-xs text-muted-foreground hover:text-foreground"
                :title="`Reveal ${row.folder}`"
                @click="openPath(row.folder)"
              >
                <FolderOpen class="size-3.5 shrink-0" />
                <span class="truncate font-mono">{{ row.folder }}</span>
              </button>
              <div class="flex shrink-0 items-center gap-2">
                <Badge variant="secondary">
                  {{ row.count }} {{ row.count === 1 ? "site" : "sites" }}
                </Badge>
                <Spinner v-if="rowBusy === `unpark:${row.folder}`" class="size-4" />
                <Button
                  v-else
                  variant="ghost"
                  size="icon"
                  :aria-label="`Un-park ${row.folder}`"
                  title="Un-park"
                  @click="openUnpark(row.folder)"
                >
                  <FolderMinus class="size-4" />
                </Button>
              </div>
            </div>
          </div>
        </section>
      </AsyncState>
    </div>

    <!-- create new Laravel site wizard -->
    <CreateLaravelWizard
      v-model:open="createOpen"
      :parked-folders="parked"
      :php-versions="phpVersionList"
      :default-php="defaultPhp"
      :tld="tld"
      :report="report ?? null"
      @created="onCreated"
    />

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
