<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  FolderMinus,
  FolderOpen,
  FolderPlus,
  Globe,
  Layers,
  Link2,
  Package,
  Pencil,
  Plus,
  Rocket,
  Search,
  SearchX,
  ShieldAlert,
} from "lucide-vue-next";

import CreateLaravelWizard from "@/components/site-create/CreateLaravelWizard.vue";
import CreateWordPressWizard from "@/components/site-create/CreateWordPressWizard.vue";
import SiteCard from "@/components/SiteCard.vue";
import SiteDetailsSidebar from "@/components/SiteDetailsSidebar.vue";
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
import Spinner from "@/components/ui/Spinner.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { sitesIntent } from "@/lib/shortcuts/sitesIntent";
import { matchesSiteFilter } from "@/lib/siteFilter";
import { slugifySiteName } from "@/lib/siteName";
import { useSitesGroupState } from "@/lib/sitesGroupState";
import { useDaemon } from "@/composables/useDaemon";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import {
  createGroup,
  deleteGroup,
  IpcError,
  link,
  openPath,
  park,
  pickDirectory,
  renameGroup,
  setGroupOrder,
  setPhp,
  setSecure,
  setSiteGroup,
  setWebRoot,
  setWordpressAutoLogin,
  setFrontController,
  sitesAndParked,
  startQuickTunnel,
  unlink,
  unpark,
} from "@/ipc/client";
import type { GroupsState, Site, SiteEntry } from "@/ipc/types";

const toast = useToast();
const { report } = useDaemon();

// Cached SWR resource shared with the Overview and the command palette (same
// "sites" key + fetcher), so revisits render instantly instead of flashing.
const { data, loading, error: resourceError, refresh: load } = useResource(
  "sites",
  sitesAndParked,
);
usePoll(() => load(), 5000);
const sites = computed(() => data.value?.sites ?? []);
const parked = computed(() => data.value?.parked ?? []);
// Surface a load failure only when nothing is cached to show; a failed
// background revalidation keeps the last-good list and never masks it.
const error = computed(() => (data.value ? null : (resourceError.value?.message ?? null)));
const rowBusy = ref<string | null>(null);
const siteFilter = ref("");
const filterInput = ref<InstanceType<typeof Input> | null>(null);

const tld = computed(() => report.value?.tld ?? "test");
const caTrusted = computed(() => report.value?.ca.trusted_system === true);

const sidebarTarget = ref<string | null>(null);
const sidebarSite = computed<SiteEntry | null>(() =>
  sidebarTarget.value
    ? (sites.value.find((site) => site.name === sidebarTarget.value) ?? null)
    : null,
);

function openSidebar(site: SiteEntry): void {
  sidebarTarget.value = site.name;
}

function closeSidebar(): void {
  sidebarTarget.value = null;
}

watch(sidebarSite, (site) => {
  if (!site) closeSidebar();
});

// ── site groups ──
const emptyGroups: GroupsState = { order: [], members: {} };
const groups = computed<GroupsState>(() => data.value?.groups ?? emptyGroups);
const hasGroups = computed(() => groups.value.order.length > 0);
const searching = computed(() => siteFilter.value.trim() !== "");
const { isCollapsed, toggle: toggleCollapse, rename: renameCollapse } = useSitesGroupState();

/** The synthetic bucket for ungrouped sites; never a stored group. */
const UNALLOCATED = "Unallocated";

interface GroupSection {
  name: string;
  sites: Site[];
  isUnallocated: boolean;
}

/** Whether a site-scoped mutation is in flight for `name`. Matches the exact
 *  `edit:`/`unlink:` tokens rather than a `:${name}` suffix, so a group op
 *  (`group:<name>`) can't spuriously spin a same-named site's card. */
function siteBusy(name: string): boolean {
  return rowBusy.value === `edit:${name}` || rowBusy.value === `unlink:${name}`;
}

async function toggleSecure(site: Site): Promise<void> {
  const next = !site.secure;
  rowBusy.value = `edit:${site.name}`;
  try {
    await setSecure(site.name, next);
    toast.success(
      next
        ? `HTTPS enabled for ${site.name}.${tld.value}`
        : `HTTPS disabled for ${site.name}.${tld.value}`,
    );
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change HTTPS", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function changeSitePhp(site: SiteEntry, version: string): Promise<void> {
  if (version === site.php) return;
  rowBusy.value = `edit:${site.name}`;
  try {
    await setPhp(site.name, version);
    toast.success(`PHP ${version} selected for ${site.name}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change PHP version", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function changeSiteWebRoot(site: SiteEntry, path: string): Promise<void> {
  const current = site.web_subpath ?? "";
  if (path === current) return;
  rowBusy.value = `edit:${site.name}`;
  try {
    await setWebRoot(site.name, path === "" ? null : path);
    toast.success(path === "" ? `Web root reset for ${site.name}` : `Web root changed for ${site.name}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change web root", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function toggleFrontController(site: SiteEntry, enabled: boolean): Promise<void> {
  if (enabled === site.uses_front_controller) return;
  rowBusy.value = `edit:${site.name}`;
  try {
    await setFrontController(site.name, enabled);
    toast.success(
      enabled
        ? `Front controller enabled for ${site.name}`
        : `Front controller disabled for ${site.name}`,
    );
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change front controller", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function moveGroup(name: string, dir: -1 | 1): Promise<void> {
  const order = [...groups.value.order];
  const i = order.indexOf(name);
  const j = i + dir;
  const a = order[i];
  const b = order[j];
  if (a === undefined || b === undefined) return;
  order[i] = b;
  order[j] = a;
  rowBusy.value = `group:${name}`;
  try {
    await setGroupOrder(order);
  } catch (e) {
    toast.error("Couldn't reorder groups", (e as IpcError).message);
  } finally {
    // Always reconcile with the daemon's actual group set - on a permutation
    // rejection (a group added/removed in another window) this drops the stale
    // section instead of leaving it stuck until an unrelated refresh.
    await load({ force: true });
    rowBusy.value = null;
  }
}

// ASCII-only case-fold, matching the daemon's `eq_ignore_ascii_case` group
// identity; full-Unicode `toLowerCase` disagrees on non-ASCII names.
function asciiLower(s: string): string {
  return s.replace(/[A-Z]/g, (c) => String.fromCharCode(c.charCodeAt(0) + 32));
}

// ── create group ──
const createGroupOpen = ref(false);
const createGroupName = ref("");
const createGroupValid = computed(() => {
  const n = asciiLower(createGroupName.value.trim());
  if (!n || n === asciiLower(UNALLOCATED)) return false;
  return !groups.value.order.some((g) => asciiLower(g) === n);
});

function openCreateGroup(): void {
  createGroupName.value = "";
  void nextTick(() => {
    createGroupOpen.value = true;
  });
}

async function confirmCreateGroup(close: () => void): Promise<void> {
  const name = createGroupName.value.trim();
  close();
  if (!name) return;
  rowBusy.value = "group:create";
  try {
    await createGroup(name);
    toast.success(`Created group ${name}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't create group", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

// ── rename / delete group (one modal: delete is a second step of the same
// "manage group" dialog rather than its own header button + popup) ──
const renameGroupOpen = ref(false);
const renameGroupTarget = ref<string | null>(null);
const renameGroupName = ref("");
const renameGroupConfirmingDelete = ref(false);
const renameGroupValid = computed(() => {
  const n = asciiLower(renameGroupName.value.trim());
  if (!n || n === asciiLower(UNALLOCATED)) return false;
  const from = renameGroupTarget.value ? asciiLower(renameGroupTarget.value) : "";
  return !groups.value.order.some((g) => {
    const gl = asciiLower(g);
    return gl === n && gl !== from;
  });
});

function openRenameGroup(name: string): void {
  renameGroupTarget.value = name;
  renameGroupName.value = name;
  renameGroupConfirmingDelete.value = false;
  void nextTick(() => {
    renameGroupOpen.value = true;
  });
}

// Validity is captured before close(): Modal's close emits update:open, whose
// listener nulls renameGroupTarget synchronously, which would otherwise flip the
// renameGroupValid computed to false for a case-only rename.
async function confirmRenameGroup(close: () => void): Promise<void> {
  const from = renameGroupTarget.value;
  const to = renameGroupName.value.trim();
  const wasValid = renameGroupValid.value;
  close();
  if (!from || !to || !wasValid || to === from) return;
  rowBusy.value = `group:${from}`;
  try {
    await renameGroup(from, to);
    renameCollapse(from, to);
    toast.success(`Renamed group ${from} to ${to}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't rename group", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function confirmDeleteGroup(close: () => void): Promise<void> {
  const name = renameGroupTarget.value;
  close();
  if (!name) return;
  rowBusy.value = `group:${name}`;
  try {
    await deleteGroup(name);
    toast.success(`Deleted group ${name}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't delete group", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

/** A site's current, sanitised group membership: the canonical `order` casing
 *  (matched ASCII-case-insensitively, like the daemon), or "" when ungrouped or
 *  the stored group no longer exists. */
function currentGroupOf(name: string): string {
  const g = groups.value.members[name];
  if (!g) return "";
  return groups.value.order.find((o) => asciiLower(o) === asciiLower(g)) ?? "";
}

const groupSelectOptions = computed(() => [
  { value: "", label: "No group" },
  ...groups.value.order.map((g) => ({ value: g, label: g })),
]);

// ── create new site ──
const createOpen = ref(false);
const wordpressCreateOpen = ref(false);
const phpVersionList = computed(() => (report.value?.php ?? []).map((p) => p.version));
const defaultPhp = computed(() => report.value?.default_php ?? "");

function openCreate(): void {
  // Defer past the dropdown's close so reka-ui's focus-restore doesn't fight the modal.
  void nextTick(() => {
    createOpen.value = true;
  });
}

function openCreateWordpress(): void {
  void nextTick(() => {
    wordpressCreateOpen.value = true;
  });
}

async function onCreated(): Promise<void> {
  await load({ force: true });
}

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

// Live, case-insensitive filter across every domain a site serves - its apex
// plus any added domains, subdomains and wildcards - sorted alphabetically by
// name so the list is stable and scannable.
const filteredSites = computed(() => {
  const q = siteFilter.value.trim();
  const list = q
    ? sites.value.filter((s) => matchesSiteFilter(s, tld.value, q))
    : [...sites.value];
  return list.sort((a, b) => a.name.localeCompare(b.name));
});

// One section per group (in order) plus a trailing synthetic "Unallocated"
// section for sites with no membership (or a membership pointing at a group that
// no longer exists - defensive). Each section keeps filteredSites' sort order.
const groupSections = computed<GroupSection[]>(() => {
  const { order, members } = groups.value;
  const known = new Set(order.map(asciiLower));
  const sections: GroupSection[] = order.map((name) => {
    const folded = asciiLower(name);
    return {
      name,
      isUnallocated: false,
      sites: filteredSites.value.filter((s) => {
        const g = members[s.name];
        return g !== undefined && asciiLower(g) === folded;
      }),
    };
  });
  const unallocated = filteredSites.value.filter((s) => {
    const g = members[s.name];
    return !g || !known.has(asciiLower(g));
  });
  sections.push({ name: UNALLOCATED, isUnallocated: true, sites: unallocated });
  return sections;
});

// While searching, hide any section with no matching sites. Unallocated is also
// hidden whenever empty (it has no controls, so an empty header is pure noise).
// Empty named groups otherwise stay visible so they remain manageable.
const visibleSections = computed(() =>
  groupSections.value.filter((sec) => {
    if (sec.isUnallocated) return sec.sites.length > 0;
    if (searching.value) return sec.sites.length > 0;
    return true;
  }),
);

// Searching that matches nothing across every group → show the same "No sites
// match" copy as the flat view instead of a blank pane.
const groupedNoMatch = computed(() => searching.value && visibleSections.value.length === 0);

/** Shared by the flat and grouped listings, which each render their own
 *  filter-miss state but must word it identically. */
const filterMissTitle = computed(() => `No sites match “${siteFilter.value}”`);

/** A section renders expanded when searching (to reveal matches) or when not
 *  remembered-collapsed. */
function sectionExpanded(sec: GroupSection): boolean {
  return searching.value || !isCollapsed(sec.name);
}

async function changeSiteGroup(site: SiteEntry, group: string): Promise<void> {
  if (group === currentGroupOf(site.name)) return;
  rowBusy.value = `edit:${site.name}`;
  try {
    await setSiteGroup(site.name, group === "" ? null : group);
    toast.success(
      group === "" ? `Removed ${site.name} from its group` : `Moved ${site.name} to ${group}`,
    );
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change group", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function changeWpAutoLogin(
  site: SiteEntry,
  enabled: boolean,
  user: string | null,
  options?: { silent?: boolean },
): Promise<void> {
  if (enabled === (site.wp_auto_login ?? false) && (user ?? "") === (site.wp_auto_login_user ?? "")) {
    return;
  }
  rowBusy.value = `edit:${site.name}`;
  try {
    await setWordpressAutoLogin(site.name, enabled, user);
    if (!options?.silent) {
      toast.success(
        enabled
          ? `Auto-login enabled for ${site.name}`
          : `Auto-login disabled for ${site.name}`,
      );
    }
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change auto-login", (e as IpcError).message);
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
    await load({ force: true });
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
    await load({ force: true });
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
// True once the user types in the name field, so choosing a folder never
// clobbers a name they entered themselves. Native `input` events fire only on
// real keystrokes, not on the programmatic prefill in `chooseLinkDir`.
const linkNameDirty = ref(false);
// A folder-derived or typed name is converted to a valid slug on submit, so
// validity is "does it slugify to something", not the raw `[a-z0-9-]` shape.
const linkValid = computed(
  () => slugifySiteName(linkName.value) !== null && linkPath.value.trim() !== "",
);

watch(linkOpen, (open) => {
  if (!open) {
    linkName.value = "";
    linkPath.value = "";
    linkNameDirty.value = false;
  }
});

function baseName(path: string): string {
  const trimmed = path.replace(/[/\\]+$/, "");
  const parts = trimmed.split(/[/\\]/);
  return parts[parts.length - 1] ?? "";
}

async function chooseLinkDir(): Promise<void> {
  const dir = await pickDirectory();
  if (!dir) return;
  linkPath.value = dir;
  if (!linkNameDirty.value) linkName.value = slugifySiteName(baseName(dir)) ?? "";
}

async function confirmLink(close: () => void): Promise<void> {
  const name = slugifySiteName(linkName.value);
  const path = linkPath.value.trim();
  if (!name) return;
  close();
  rowBusy.value = "link";
  try {
    await link(name, path);
    toast.success(`Linked ${name}`);
    await load({ force: true });
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
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't remove site", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
    unlinkTarget.value = null;
  }
}

onUnmounted(
  registerViewActions({
    create: openCreate,
    find: () => filterInput.value?.focus(),
    refresh: () => void load(),
  }),
);

function consumeIntent(): void {
  const intent = sitesIntent.value;
  if (!intent) return;
  sitesIntent.value = null;
  if (intent === "link") linkOpen.value = true;
  else if (intent === "create") openCreate();
  else if (intent === "park") void onPark();
}

onMounted(consumeIntent);
watch(sitesIntent, consumeIntent);

/**
 * Publish a site over a Cloudflare Quick Tunnel. The tunnel is then managed
 * (and stopped) from the Integrations page; here we just kick it off and surface
 * the public URL. A missing `cloudflared` surfaces as a daemon error toast that
 * points at Integrations.
 */
const sharing = ref<string | null>(null);
async function shareSitePublicly(s: Site): Promise<void> {
  sharing.value = s.name;
  try {
    const r = await startQuickTunnel(s.name);
    const url = r.tunnels.find((t) => t.site === s.name)?.url;
    if (url) {
      const copied = await navigator.clipboard
        .writeText(url)
        .then(() => true)
        .catch(() => false);
      toast.success(
        `Sharing ${s.name}`,
        copied ? `${url} (copied) - manage in Integrations` : `${url} - manage in Integrations`,
      );
    } else {
      toast.success(`Sharing ${s.name}`, "Starting - see Integrations for the URL");
    }
  } catch (e) {
    toast.error(`Couldn't share ${s.name}`, (e as IpcError).message);
  } finally {
    sharing.value = null;
  }
}
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Sites" subtitle="Parked and linked .test sites" docs="/guide/sites">
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
            <DropdownMenuItem @select="openCreateWordpress">
              <Package class="size-4" /> New WordPress site…
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem @select="linkOpen = true">
              <Link2 class="size-4" /> Link existing site
            </DropdownMenuItem>
            <DropdownMenuItem @select="onPark">
              <FolderPlus class="size-4" /> Park folder
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem @select="openCreateGroup">
              <Layers class="size-4" /> New group…
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
        :empty="sites.length === 0 && parked.length === 0 && !hasGroups"
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
              ref="filterInput"
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

        <!-- Flat listing (no groups defined): the classic card grid. -->
        <template v-if="!hasGroups">
          <div
            v-if="filteredSites.length"
            class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3"
          >
            <SiteCard
              v-for="s in filteredSites"
              :key="s.name"
              :site="s"
              :report="report ?? null"
              :tld="tld"
              :busy="siteBusy(s.name)"
              @edit="openSidebar"
              @toggle-secure="toggleSecure"
            />
          </div>
          <EmptyState
            v-else-if="siteFilter"
            :icon="SearchX"
            :title="filterMissTitle"
            description="Every domain a site serves is searched, including added domains and wildcards. Try a shorter fragment, or clear the filter."
          />
          <EmptyState
            v-else
            :icon="FolderOpen"
            title="No sites yet"
            description="Your parked folders have no child directories. Add a project inside one, or link a single project directory."
          />
        </template>

        <!-- Grouped listing: collapsible sections + trailing Unallocated. -->
        <template v-else>
          <div class="space-y-3">
            <section
              v-for="sec in visibleSections"
              :key="sec.name"
              class="rounded-lg border"
            >
              <div class="group flex items-center gap-2 px-3 py-2.5">
                <button
                  class="flex min-w-0 flex-1 items-center gap-2 text-left"
                  :aria-expanded="sectionExpanded(sec)"
                  @click="searching ? undefined : toggleCollapse(sec.name)"
                >
                  <component
                    :is="sectionExpanded(sec) ? ChevronDown : ChevronRight"
                    class="size-4 shrink-0 text-muted-foreground"
                  />
                  <span class="truncate text-sm font-semibold">{{ sec.name }}</span>
                  <Badge variant="secondary">{{ sec.sites.length }}</Badge>
                </button>
                <!-- Reorder + edit controls (named groups only; hidden while
                     searching, since a search-hidden neighbour would make a move
                     look like a no-op). Faded in only on hover/focus of this row
                     so the header matches Unallocated's height at rest. -->
                <div
                  v-if="!sec.isUnallocated && !searching"
                  class="flex shrink-0 items-center gap-0.5"
                >
                  <Spinner v-if="rowBusy === `group:${sec.name}`" class="size-4" />
                  <div
                    v-else
                    class="flex items-center gap-0.5 pointer-events-none opacity-0 transition-opacity duration-300 group-hover:pointer-events-auto group-hover:opacity-70 focus-within:pointer-events-auto focus-within:opacity-70"
                  >
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :disabled="groups.order.indexOf(sec.name) === 0"
                      :aria-label="`Move ${sec.name} up`"
                      title="Move up"
                      @click="moveGroup(sec.name, -1)"
                    >
                      <ArrowUp class="size-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :disabled="groups.order.indexOf(sec.name) === groups.order.length - 1"
                      :aria-label="`Move ${sec.name} down`"
                      title="Move down"
                      @click="moveGroup(sec.name, 1)"
                    >
                      <ArrowDown class="size-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      :aria-label="`Edit ${sec.name}`"
                      title="Edit group"
                      @click="openRenameGroup(sec.name)"
                    >
                      <Pencil class="size-4" />
                    </Button>
                  </div>
                </div>
              </div>
              <div v-if="sectionExpanded(sec)" class="border-t p-3">
                <div
                  v-if="sec.sites.length"
                  class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3"
                >
                  <SiteCard
                    v-for="s in sec.sites"
                    :key="s.name"
                    :site="s"
                    :report="report ?? null"
                    :tld="tld"
                    :busy="siteBusy(s.name)"
                    @edit="openSidebar"
                    @toggle-secure="toggleSecure"
                  />
                </div>
                <p v-else class="py-4 text-center text-xs text-muted-foreground">
                  No sites in this group yet.
                </p>
              </div>
            </section>
          </div>
          <EmptyState
            v-if="groupedNoMatch"
            :icon="SearchX"
            :title="filterMissTitle"
            description="Every domain a site serves is searched, including added domains and wildcards. Try a shorter fragment, or clear the filter."
          />
        </template>

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

    <SiteDetailsSidebar
      :site="sidebarSite"
      :open="sidebarSite !== null"
      :report="report ?? null"
      :tld="tld"
      :php-versions="phpVersionList"
      :group-options="hasGroups ? groupSelectOptions : []"
      :current-group="sidebarSite ? currentGroupOf(sidebarSite.name) : ''"
      :busy="sidebarSite ? siteBusy(sidebarSite.name) : false"
      :sharing="sidebarSite ? sharing === sidebarSite.name : false"
      @close="closeSidebar"
      @change-php="changeSitePhp"
      @change-web-root="changeSiteWebRoot"
      @toggle-secure="toggleSecure"
      @toggle-front-controller="toggleFrontController"
      @change-group="changeSiteGroup"
      @change-wp-auto-login="changeWpAutoLogin"
      @share="shareSitePublicly"
      @unlink="openUnlink"
      @domains-changed="load({ force: true })"
    />

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

    <!-- create new WordPress site wizard -->
    <CreateWordPressWizard
      v-model:open="wordpressCreateOpen"
      :parked-folders="parked"
      :php-versions="phpVersionList"
      :default-php="defaultPhp"
      :tld="tld"
      :report="report ?? null"
      @created="onCreated"
    />

    <!-- link modal -->
    <Modal v-model:open="linkOpen" title="Link a site">
      <label class="text-sm font-medium" for="linkname">Name (single label)</label>
      <Input
        id="linkname"
        v-model="linkName"
        placeholder="e.g. myapp"
        class="mt-2"
        @input="linkNameDirty = true"
      />
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

    <!-- create group -->
    <Modal v-model:open="createGroupOpen" title="New group">
      <label class="text-sm font-medium" for="groupname">Group name</label>
      <Input
        id="groupname"
        v-model="createGroupName"
        placeholder="e.g. Client work"
        class="mt-2"
        @keyup.enter="createGroupValid && confirmCreateGroup(() => (createGroupOpen = false))"
      />
      <p class="mt-1 text-xs text-muted-foreground">
        Sites can be assigned to a group when you edit them.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button :disabled="!createGroupValid" @click="confirmCreateGroup(close)">Create</Button>
      </template>
    </Modal>

    <!-- edit group: rename, or a second step to delete -->
    <Modal
      v-model:open="renameGroupOpen"
      :title="renameGroupConfirmingDelete ? 'Delete group' : 'Edit group'"
      @update:open="
        (v: boolean) => {
          if (!v) {
            renameGroupTarget = null;
            renameGroupConfirmingDelete = false;
          }
        }
      "
    >
      <template v-if="!renameGroupConfirmingDelete">
        <label class="text-sm font-medium" for="renamegroupname">Group name</label>
        <Input
          id="renamegroupname"
          v-model="renameGroupName"
          placeholder="e.g. Client work"
          class="mt-2"
          @keyup.enter="renameGroupValid && confirmRenameGroup(() => (renameGroupOpen = false))"
        />
      </template>
      <p v-else class="text-sm text-muted-foreground">
        Delete <strong class="text-foreground">{{ renameGroupTarget }}</strong>? Its
        sites aren't removed - they move to <strong class="text-foreground">Unallocated</strong>.
      </p>
      <template #footer="{ close }">
        <template v-if="!renameGroupConfirmingDelete">
          <Button
            variant="ghost"
            class="mr-auto text-destructive hover:text-destructive"
            @click="renameGroupConfirmingDelete = true"
          >
            Delete group
          </Button>
          <Button variant="ghost" @click="close">Cancel</Button>
          <Button :disabled="!renameGroupValid" @click="confirmRenameGroup(close)">Save</Button>
        </template>
        <template v-else>
          <Button variant="ghost" @click="renameGroupConfirmingDelete = false">Back</Button>
          <Button variant="destructive" @click="confirmDeleteGroup(close)">Delete group</Button>
        </template>
      </template>
    </Modal>
  </div>
</template>
