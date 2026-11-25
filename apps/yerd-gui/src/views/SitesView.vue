<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import { FolderMinus, FolderPlus, Layers, Plus, SearchX, Trash2 } from "lucide-vue-next";

import NewSiteDialog from "@/components/site-create/NewSiteDialog.vue";
import LinkFolderDialog, { type LinkRequest } from "@/components/sites/LinkFolderDialog.vue";
import SiteFilterChips from "@/components/sites/SiteFilterChips.vue";
import SiteInspector from "@/components/sites/SiteInspector.vue";
import SitesEmptyState from "@/components/sites/SitesEmptyState.vue";
import SitesList from "@/components/sites/SitesList.vue";
import SitesWarningStrip from "@/components/sites/SitesWarningStrip.vue";
import PageHeader from "@/components/PageHeader.vue";
import AsyncState from "@/components/ui/AsyncState.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { sitesIntent } from "@/lib/shortcuts/sitesIntent";
import { matchesSiteFilter } from "@/lib/siteFilter";
import { slugifySiteName } from "@/lib/siteName";
import {
  asciiLower,
  buildSections,
  type ChipKey,
  groupNameAvailable,
  matchesChip,
  parentDir,
  siteAtPath,
  siteChips,
  siteWarnings,
  tildify,
} from "@/lib/sites";
import { useSitesGroupState } from "@/lib/sitesGroupState";
import { siteHost, siteUrl } from "@/lib/siteUrl";
import { copyUrlWithToast, toastOnError } from "@/lib/toastActions";
import { plural } from "@/lib/utils";
import { useDaemon } from "@/composables/useDaemon";
import { useOperations } from "@/composables/useOperations";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import { openSiteInEditor } from "@/composables/useSiteEditor";
import { useToast } from "@/composables/useToast";
import { useWideLayout } from "@/composables/useWideLayout";
import {
  applyProjectConfig,
  createGroup,
  deleteGroup,
  exportProjectConfig,
  IpcError,
  link,
  nodeSites,
  openInBrowser,
  openInTerminal,
  openPath,
  park,
  pickDirectory,
  pollJobToEnd,
  renameGroup,
  setGroupOrder,
  setPhp,
  setSecure,
  setSiteFavourite,
  setSiteGroup,
  setWebRoot,
  setWordpressAutoLogin,
  setFrontController,
  sitesAndParked,
  listTunnelProviders,
  startQuickTunnel,
  stopTunnel,
  tunnelStatus,
  unlink,
  unpark,
} from "@/ipc/client";
import type { GroupsState, Site, SiteEntry } from "@/ipc/types";
import { CLEANUP_UNKNOWN_MESSAGE, checkProviderStart, cleanupFailedMessage, siteShare, unsupportedProviderMessage } from "@/lib/share";

const toast = useToast();
const operations = useOperations();
const { report } = useDaemon();
const { home, supportsPathInstall } = usePlatform();

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

const tld = computed(() => report.value?.tld ?? "test");
const phpVersionList = computed(() => (report.value?.php ?? []).map((p) => p.version));
const defaultPhp = computed(() => report.value?.default_php ?? "");
const warnings = computed(() => siteWarnings(report.value));

// ── layout: list + inspector side by side when wide, table + drawer otherwise ──
const wide = useWideLayout();

// ── Node column (table only): loaded when the table shows and after mutations ──
const { data: nodeData, refresh: refreshNodeSites } = useResource("node-sites", nodeSites, {
  immediate: false,
});
const nodeVersions = computed<Record<string, string>>(() =>
  Object.fromEntries(
    (nodeData.value ?? [])
      .filter((n) => n.effective)
      .map((n) => [n.site, n.effective as string]),
  ),
);
watch(
  wide,
  (isWide) => {
    if (!isWide) void refreshNodeSites();
  },
  { immediate: true },
);

/** Refetch the Node column, only while the table shows it. */
function refreshNodeColumn(): void {
  if (!wide.value) void refreshNodeSites({ force: true });
}

/** Refetch after a write: the site list always, the Node column when it shows. */
async function reload(): Promise<void> {
  await load({ force: true });
  refreshNodeColumn();
}

/** Whether a site-scoped mutation is in flight for `name`. Matches the exact
 *  `edit:`/`unlink:` tokens rather than a `:${name}` suffix, so a group op
 *  (`group:<name>`) can't spuriously spin a same-named site's row. */
function siteBusy(name: string): boolean {
  return rowBusy.value === `edit:${name}` || rowBusy.value === `unlink:${name}`;
}

// ── site groups ──
const emptyGroups: GroupsState = { order: [], members: {} };
const groups = computed<GroupsState>(() => data.value?.groups ?? emptyGroups);
const hasGroups = computed(() => groups.value.order.length > 0);
const { rename: renameCollapse } = useSitesGroupState();

/** A site's current, sanitised group membership: the canonical `order` casing
 *  (matched ASCII-case-insensitively, like the daemon), or "" when ungrouped or
 *  the stored group no longer exists. */
function currentGroupOf(name: string): string {
  const g = groups.value.members[name];
  if (!g) return "";
  return groups.value.order.find((o) => asciiLower(o) === asciiLower(g)) ?? "";
}

// ── filtering ──
const activeChip = ref<ChipKey>("all");
const siteFilter = ref("");
const chipsRef = ref<InstanceType<typeof SiteFilterChips> | null>(null);
const chips = computed(() => siteChips(sites.value, defaultPhp.value));
const filtering = computed(() => siteFilter.value.trim() !== "" || activeChip.value !== "all");

watch(chips, (list) => {
  if (!list.some((c) => c.key === activeChip.value)) activeChip.value = "all";
});

const filteredSites = computed(() =>
  sites.value
    .filter((s) => matchesChip(s, activeChip.value, defaultPhp.value))
    .filter((s) => matchesSiteFilter(s, tld.value, siteFilter.value, home.value)),
);

const sections = computed(() =>
  buildSections({
    sites: filteredSites.value,
    groups: groups.value,
    parked: parked.value,
    filtering: filtering.value,
  }),
);

const visibleNames = computed(() => sections.value.flatMap((s) => s.sites.map((x) => x.name)));
const noMatch = computed(() => filtering.value && visibleNames.value.length === 0);
const isEmpty = computed(
  () => sites.value.length === 0 && parked.value.length === 0 && !hasGroups.value,
);
const createSlug = computed(() => slugifySiteName(siteFilter.value.trim()));

function clearFilter(): void {
  siteFilter.value = "";
  activeChip.value = "all";
}

// ── selection ──
const selectedName = ref<string | null>(null);
const drawerOpen = ref(false);
const selectedSite = computed<SiteEntry | null>(() =>
  selectedName.value ? (sites.value.find((s) => s.name === selectedName.value) ?? null) : null,
);

/** The wide layout always inspects a site: it picks the first visible one when
 *  nothing is selected or the selection no longer exists. A selection that a
 *  chip or query merely hides stays put, so an edit that drops the site out of
 *  the filter never retargets the inspector (and the next click) to another. */
watch(
  [visibleNames, wide, selectedSite],
  ([names, isWide, current]) => {
    if (!isWide || current) return;
    selectedName.value = names[0] ?? null;
  },
  { immediate: true },
);

watch(selectedSite, (site) => {
  if (!site) drawerOpen.value = false;
});

watch(wide, (isWide) => {
  if (isWide) drawerOpen.value = false;
});

function selectSite(name: string): void {
  selectedName.value = name;
}

function activateSite(name: string): void {
  selectedName.value = name;
  drawerOpen.value = true;
}

/** The parked root serving a site, or null for a linked or unmatched one. */
function parentDirOf(site: SiteEntry): string | null {
  if (site.kind !== "parked") return null;
  const dir = parentDir(site.document_root);
  return parked.value.includes(dir) ? dir : null;
}

function siteByName(name: string): SiteEntry | undefined {
  return sites.value.find((s) => s.name === name);
}

function toggleFavouriteByName(name: string): void {
  const site = siteByName(name);
  if (site) void toggleFavourite(site);
}

// ── list keyboard actions ──
async function openSite(name: string): Promise<void> {
  const site = siteByName(name);
  if (!site) return;
  await toastOnError("Couldn't open site", () => openInBrowser(siteUrl(site, report.value)));
}

function openEditorFor(name: string): void {
  if (supportsPathInstall.value) void openSiteInEditor(name);
}

async function openTerminalFor(name: string): Promise<void> {
  const site = siteByName(name);
  if (!site) return;
  await toastOnError("Couldn't open terminal", () => openInTerminal(site.document_root));
}

async function copyUrlOf(name: string): Promise<void> {
  const site = siteByName(name);
  if (!site) return;
  await copyUrlWithToast(siteUrl(site, report.value));
}

async function revealDir(dir: string): Promise<void> {
  await toastOnError("Couldn't reveal folder", () => openPath(dir));
}

// ── site mutations ──
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
    await reload();
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
    await reload();
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
    await reload();
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
    await reload();
  } catch (e) {
    toast.error("Couldn't change front controller", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

async function toggleFavourite(site: SiteEntry): Promise<void> {
  const next = !site.favourite;
  rowBusy.value = `edit:${site.name}`;
  try {
    await setSiteFavourite(site.name, next);
    toast.success(next ? `Starred ${site.name}` : `Unstarred ${site.name}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change favourite", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

/** Swap a group with its neighbour. It always reconciles with the daemon's
 *  actual group set afterwards: on a permutation rejection (a group added or
 *  removed in another window) that drops the stale section instead of leaving it
 *  stuck until an unrelated refresh. */
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
    await load({ force: true });
    rowBusy.value = null;
  }
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

/** New group… from Move to group: create it, then move the site into it. */
async function createGroupAndMove(site: SiteEntry, name: string): Promise<void> {
  rowBusy.value = `edit:${site.name}`;
  try {
    await createGroup(name);
  } catch (e) {
    toast.error("Couldn't create group", (e as IpcError).message);
    rowBusy.value = null;
    return;
  }
  try {
    await setSiteGroup(site.name, name);
    toast.success(`Moved ${site.name} to ${name}`);
  } catch (e) {
    toast.error("Couldn't change group", (e as IpcError).message);
  } finally {
    await load({ force: true });
    rowBusy.value = null;
  }
}

async function changeWpAutoLogin(
  site: SiteEntry,
  enabled: boolean,
  user: string | null,
): Promise<void> {
  if (enabled === (site.wp_auto_login ?? false) && (user ?? "") === (site.wp_auto_login_user ?? "")) {
    return;
  }
  rowBusy.value = `edit:${site.name}`;
  try {
    await setWordpressAutoLogin(site.name, enabled, user);
    toast.success(
      enabled ? `Auto-login enabled for ${site.name}` : `Auto-login disabled for ${site.name}`,
    );
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change auto-login", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

// ── create an empty group ──
const createGroupOpen = ref(false);
const createGroupName = ref("");
const createGroupValid = computed(() =>
  groupNameAvailable(createGroupName.value, groups.value.order),
);

function openCreateGroup(): void {
  createGroupName.value = "";
  void nextTick(() => {
    createGroupOpen.value = true;
  });
}

async function confirmCreateGroup(close: () => void): Promise<void> {
  const name = createGroupName.value.trim();
  const wasValid = createGroupValid.value;
  close();
  if (!name || !wasValid) return;
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
const renameGroupValid = computed(() =>
  groupNameAvailable(renameGroupName.value, groups.value.order, renameGroupTarget.value),
);

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

// ── park / un-park ──
async function onPark(): Promise<void> {
  const dir = await pickDirectory();
  if (!dir) return;
  rowBusy.value = "park";
  try {
    await park(dir);
    toast.success("Parked directory", dir);
    await reload();
  } catch (e) {
    toast.error("Park failed", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

const unparkOpen = ref(false);
const unparkTarget = ref<string | null>(null);

function openUnpark(folder: string): void {
  unparkTarget.value = folder;
  void nextTick(() => {
    unparkOpen.value = true;
  });
}

const unparkCount = computed(() =>
  unparkTarget.value
    ? sites.value.filter((s) => s.kind === "parked" && parentDir(s.document_root) === unparkTarget.value)
        .length
    : 0,
);

const unparkBody = computed(() => {
  const n = unparkCount.value;
  const rest = "Your files stay where they are, and linked sites aren't affected.";
  if (n === 0) return `Nothing in this folder is being served yet. ${rest}`;
  if (n === 1) return `1 site in this folder stops being served. ${rest}`;
  return `${n} sites in this folder stop being served. ${rest}`;
});

const unparkLabel = computed(() => {
  const n = unparkCount.value;
  if (n === 0) return "Unpark folder";
  return `Unpark ${plural(n, "site", "sites")}`;
});

async function confirmUnpark(): Promise<void> {
  const folder = unparkTarget.value;
  if (!folder) return;
  rowBusy.value = `unpark:${folder}`;
  try {
    await unpark(folder);
    toast.success("Un-parked folder", folder);
    await reload();
  } catch (e) {
    toast.error("Un-park failed", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
    unparkTarget.value = null;
  }
}

// ── link ──
const linkOpen = ref(false);

function openLink(): void {
  void nextTick(() => {
    linkOpen.value = true;
  });
}

/** Link the folder, then apply the PHP version and HTTPS the dialog asked for
 *  (if any: `null` when an unapplied `.yerd.toml` keeps the defaults) where
 *  they differ from what the daemon assigned. Each follow-up failure is
 *  reported on its own; the site stays linked either way. */
async function onLink(request: LinkRequest): Promise<void> {
  const { name, path, php, secure } = request;
  rowBusy.value = "link";
  try {
    if (request.applyProjectFile) {
      await applyProjectFile(path, name);
      const linked = siteAtPath(sites.value, path);
      if (linked) selectedName.value = linked.name;
      return;
    }
    try {
      await link(name, path);
    } catch (e) {
      toast.error("Link failed", (e as IpcError).message);
      return;
    }
    toast.success(`Linked ${name}`);
    await reload();
    selectedName.value = name;
    const site = siteByName(name);
    if (!site) return;
    let changed = false;
    if (php && site.php !== php) {
      try {
        await setPhp(name, php);
        changed = true;
      } catch (e) {
        toast.error("Couldn't change PHP version", (e as IpcError).message);
      }
    }
    if (secure !== null && site.secure !== secure) {
      try {
        await setSecure(name, secure);
        changed = true;
      } catch (e) {
        toast.error("Couldn't change HTTPS", (e as IpcError).message);
      }
    }
    if (changed) await reload();
  } finally {
    rowBusy.value = null;
  }
}

// ── project file (.yerd.toml) ──
const WARNING_PREFIX = "warning:";
const FAILED_PREFIX = "failed:";

function stripPrefix(line: string, prefix: string): string {
  return line.slice(prefix.length).trim();
}

/** Apply the `.yerd.toml` in `path` as a daemon job, tracked as an operation,
 *  then toast the outcome: success, success with the job's warnings, or the
 *  job's error with its failed items. Resolves true when the job succeeded. */
async function applyProjectFile(path: string, label: string): Promise<boolean> {
  const opId = `project-apply:${path}`;
  operations.begin({ id: opId, kind: "project-apply", label: `Applying .yerd.toml for ${label}` });
  const warnings: string[] = [];
  const failures: string[] = [];
  try {
    const jobId = await applyProjectConfig(path);
    const final = await pollJobToEnd(
      jobId,
      (lines) => {
        for (const line of lines) {
          if (line.startsWith(WARNING_PREFIX)) warnings.push(stripPrefix(line, WARNING_PREFIX));
          if (line.startsWith(FAILED_PREFIX)) failures.push(stripPrefix(line, FAILED_PREFIX));
        }
        const latest = lines[lines.length - 1];
        if (latest) operations.update(opId, { detail: latest });
      },
      undefined,
      500,
      5,
    );
    if (final.state !== "succeeded") {
      const reason = final.error || `apply ${final.state}`;
      toast.error("Couldn't apply .yerd.toml", [reason, ...failures].join(". "));
      return false;
    }
    if (warnings.length) {
      toast.info(
        `Applied .yerd.toml for ${label} with ${plural(warnings.length, "warning", "warnings")}`,
        warnings.join(". "),
        10000,
      );
    } else {
      toast.success(`Applied .yerd.toml for ${label}`);
    }
    return true;
  } catch (e) {
    toast.error("Couldn't apply .yerd.toml", (e as IpcError).message);
    return false;
  } finally {
    operations.end(opId);
    await reload();
  }
}

async function applySiteProjectFile(site: SiteEntry): Promise<void> {
  rowBusy.value = `edit:${site.name}`;
  try {
    await applyProjectFile(site.document_root, site.name);
  } finally {
    rowBusy.value = null;
  }
}

async function exportSiteProjectFile(site: SiteEntry): Promise<void> {
  try {
    const written = await exportProjectConfig(site.name);
    toast.success(`Exported ${site.name} to .yerd.toml`, tildify(written.file, home.value));
  } catch (e) {
    toast.error("Couldn't export .yerd.toml", (e as IpcError).message);
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

async function confirmUnlink(): Promise<void> {
  const s = unlinkTarget.value;
  if (!s) return;
  rowBusy.value = `unlink:${s.name}`;
  try {
    await unlink(s.name);
    toast.success(`Removed ${s.name}`);
    await reload();
  } catch (e) {
    toast.error("Couldn't remove site", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
    unlinkTarget.value = null;
  }
}

// ── new site ──
const newSiteOpen = ref(false);
const siteNames = computed(() => sites.value.map((s) => s.name));
const newSiteName = ref<string | undefined>(undefined);

/** Open New site, optionally prefilled. Deferred a tick so a closing menu's
 *  focus restore doesn't fight the dialog. */
function openCreate(opts?: { name?: string }): void {
  newSiteName.value = opts?.name;
  void nextTick(() => {
    newSiteOpen.value = true;
  });
}

async function onCreated(): Promise<void> {
  await reload();
}

onUnmounted(
  registerViewActions({
    create: () => openCreate(),
    find: () => chipsRef.value?.focus(),
    refresh: () => void load(),
  }),
);

function consumeIntent(): void {
  const intent = sitesIntent.value;
  if (!intent) return;
  sitesIntent.value = null;
  if (intent === "link") openLink();
  else if (intent === "create") openCreate();
  else if (intent === "park") void onPark();
}

onMounted(() => {
  void loadPlatform();
  consumeIntent();
});
watch(sitesIntent, consumeIntent);

/**
 * Publish a site over a Cloudflare Quick Tunnel. The tunnel is then managed
 * (and stopped) from the Integrations page; here we just kick it off and surface
 * the public URL. A missing `cloudflared` surfaces as a daemon error toast that
 * points at Integrations. An older daemon that ignores the default provider and
 * starts Cloudflare instead is reported as an error, and a Cloudflare share it
 * created is stopped again.
 */
const sharing = ref<string | null>(null);
async function shareSitePublicly(s: Site): Promise<void> {
  sharing.value = s.name;
  try {
    const provider = await listTunnelProviders()
      .then((p) => p.default_provider)
      .catch(() => undefined);
    const before =
      provider && provider !== "cloudflare"
        ? await tunnelStatus()
            .then((t) => t.tunnels)
            .catch(() => null)
        : [];
    const r = await startQuickTunnel(s.name, provider);
    const check = checkProviderStart(provider, before, r.tunnels);
    const stray = check.stopCloudflare;
    const cleaned = stray
      ? await stopTunnel(stray, "cloudflare")
          .then(() => true)
          .catch(() => false)
      : true;
    if (check.unsupported && provider) {
      toast.error(`Couldn't share ${s.name}`, unsupportedProviderMessage(provider));
      if (stray && !cleaned) toast.error(`Couldn't stop the Cloudflare share of ${stray}`, cleanupFailedMessage(stray));
      if (check.cleanupUnknown) toast.error("Check your Cloudflare shares", CLEANUP_UNKNOWN_MESSAGE);
      return;
    }
    const entry = provider
      ? siteShare(r.tunnels, s.name, provider)
      : (r.tunnels.find((t) => t.site === s.name && t.kind === "quick") ?? null);
    const url = entry?.url;
    if (url) {
      const copied = await navigator.clipboard
        .writeText(url)
        .then(() => true)
        .catch(() => false);
      toast.success(
        `Sharing ${s.name}`,
        copied ? `${url} (copied) - manage in Integrations` : `${url} - manage in Integrations`,
      );
    } else if (entry?.state === "failed") {
      toast.info(
        `Sharing ${s.name}`,
        entry.last_error
          ? `Retrying: ${entry.last_error} - see Share for status`
          : "Retrying - see Share for status",
      );
    } else {
      toast.success(`Sharing ${s.name}`, "Connecting - see Share for the URL");
    }
    if (entry?.warnings?.length) {
      toast.info(`Check ${s.name} before sharing`, entry.warnings.join(" "), 10000);
    }
  } catch (e) {
    toast.error(`Couldn't share ${s.name}`, (e as IpcError).message);
  } finally {
    sharing.value = null;
  }
}

const inspectorProps = computed(() => {
  const site = selectedSite.value;
  return {
    site,
    report: report.value ?? null,
    tld: tld.value,
    home: home.value,
    phpVersions: phpVersionList.value,
    defaultPhp: defaultPhp.value,
    groups: groups.value,
    currentGroup: site ? currentGroupOf(site.name) : "",
    busy: site ? siteBusy(site.name) : false,
    sharing: site ? sharing.value === site.name : false,
    parentDir: site ? parentDirOf(site) : null,
  };
});
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Sites" docs="/guide/sites">
      <template #actions>
        <Button variant="outline" size="sm" :disabled="rowBusy === 'park'" @click="onPark">
          <FolderPlus /> Park folder
        </Button>
        <Button
          variant="outline"
          size="sm"
          :disabled="rowBusy === 'group:create'"
          @click="openCreateGroup"
        >
          <Layers /> New group
        </Button>
        <Button size="sm" @click="openCreate()"><Plus /> New site</Button>
      </template>
    </PageHeader>

    <SitesWarningStrip v-for="w in warnings" :key="w.key" :warning="w" :compact="!wide" />

    <div class="min-h-0 flex-1">
      <AsyncState :loading="loading" :error="error" :empty="isEmpty" pad="py-16" @retry="load">
        <template #empty>
          <SitesEmptyState
            :tld="tld"
            :parking="rowBusy === 'park'"
            @park="onPark"
            @create="openCreate()"
            @link="openLink"
          />
        </template>

        <div class="flex h-full min-h-0" :data-layout="wide ? 'wide' : 'compact'">
          <div class="flex min-w-0 flex-1 flex-col pl-4 pt-4">
            <SiteFilterChips
              ref="chipsRef"
              class="pr-4"
              v-model="activeChip"
              v-model:query="siteFilter"
              :chips="chips"
            />

            <div
              v-if="noMatch"
              class="flex flex-1 flex-col items-center justify-center px-6 pb-16 text-center"
              data-testid="sites-no-match"
            >
              <SearchX class="size-5 text-muted-foreground" aria-hidden="true" />
              <p class="mt-3 text-[15px] font-semibold">
                {{ siteFilter.trim() ? `Nothing matches “${siteFilter.trim()}”` : "No sites match this filter" }}
              </p>
              <p class="mt-1.5 max-w-[220px] text-[13px] text-muted-foreground">
                Search looks at names, paths and aliases.
              </p>
              <Button
                v-if="siteFilter.trim() && createSlug"
                variant="outline"
                size="sm"
                class="mt-4"
                @click="openCreate({ name: createSlug })"
              >
                <Plus /> Create {{ createSlug }}.{{ tld }}
              </Button>
              <button
                type="button"
                class="mt-3 text-[13px] font-medium text-brand hover:underline"
                @click="clearFilter"
              >
                Clear filter
              </button>
            </div>

            <SitesList
              v-else
              class="mt-1 flex-1"
              :sections="sections"
              :layout="wide ? 'list' : 'table'"
              :selected="selectedName"
              :tld="tld"
              :default-php="defaultPhp"
              :home="home"
              :node-versions="nodeVersions"
              :filtering="filtering"
              :busy="siteBusy"
              :group-busy="rowBusy"
              :group-order="groups.order"
              @select="selectSite"
              @activate="activateSite"
              @open="openSite"
              @editor="openEditorFor"
              @terminal="openTerminalFor"
              @copy-url="copyUrlOf"
              @move-group="moveGroup"
              @edit-group="openRenameGroup"
              @reveal-dir="revealDir"
              @unpark-dir="openUnpark"
              @toggle-favourite="toggleFavouriteByName"
            />
          </div>

          <div v-if="wide" class="w-[58%] max-w-[760px] shrink-0 border-l" data-testid="inspector-column">
            <SiteInspector
              v-if="selectedSite"
              mode="panel"
              v-bind="inspectorProps"
              @change-php="changeSitePhp"
              @change-web-root="changeSiteWebRoot"
              @toggle-secure="toggleSecure"
              @toggle-front-controller="toggleFrontController"
              @change-group="changeSiteGroup"
              @change-wp-auto-login="changeWpAutoLogin"
              @share="shareSitePublicly"
              @unlink="openUnlink"
              @apply-project-file="applySiteProjectFile"
              @export-project-file="exportSiteProjectFile"
              @domains-changed="reload"
              @toggle-favourite="toggleFavourite"
              @create-group-and-move="createGroupAndMove"
              @unpark="openUnpark"
              @node-changed="refreshNodeColumn"
            />
            <p v-else class="flex h-full items-center justify-center text-[13px] text-faint">
              Select a site to see its details.
            </p>
          </div>
        </div>
      </AsyncState>
    </div>

    <SiteInspector
      v-if="!wide"
      mode="drawer"
      :open="drawerOpen && selectedSite !== null"
      v-bind="inspectorProps"
      @close="drawerOpen = false"
      @change-php="changeSitePhp"
      @change-web-root="changeSiteWebRoot"
      @toggle-secure="toggleSecure"
      @toggle-front-controller="toggleFrontController"
      @change-group="changeSiteGroup"
      @change-wp-auto-login="changeWpAutoLogin"
      @share="shareSitePublicly"
      @unlink="openUnlink"
      @apply-project-file="applySiteProjectFile"
      @export-project-file="exportSiteProjectFile"
      @domains-changed="reload"
      @toggle-favourite="toggleFavourite"
      @create-group-and-move="createGroupAndMove"
      @unpark="openUnpark"
      @node-changed="refreshNodeColumn"
    />

    <NewSiteDialog
      v-model:open="newSiteOpen"
      :existing-names="siteNames"
      :parked-folders="parked"
      :php-versions="phpVersionList"
      :default-php="defaultPhp"
      :tld="tld"
      :report="report ?? null"
      :home="home"
      :initial-name="newSiteName"
      @created="onCreated"
      @existing="openLink"
    />

    <LinkFolderDialog
      v-model:open="linkOpen"
      :tld="tld"
      :home="home"
      :php-versions="phpVersionList"
      :default-php="defaultPhp"
      @link="onLink"
    />

    <ConfirmDialog
      v-model:open="unparkOpen"
      :title="`Unpark ${unparkTarget ? tildify(unparkTarget, home) : ''}?`"
      :confirm-label="unparkLabel"
      tone="danger"
      :icon="FolderMinus"
      @confirm="confirmUnpark"
    >
      {{ unparkBody }}
    </ConfirmDialog>

    <ConfirmDialog
      v-model:open="unlinkOpen"
      :title="`Remove ${unlinkTarget ? siteHost(unlinkTarget, tld) : ''}?`"
      confirm-label="Remove site"
      tone="danger"
      :icon="Trash2"
      @confirm="confirmUnlink"
    >
      Yerd stops serving it.
      <span class="font-mono">{{ unlinkTarget ? tildify(unlinkTarget.document_root, home) : "" }}</span>
      is not deleted.
    </ConfirmDialog>

    <!-- create an empty group -->
    <Modal v-model:open="createGroupOpen" title="New group">
      <label class="text-sm font-medium" for="newgroupname">Group name</label>
      <Input
        id="newgroupname"
        v-model="createGroupName"
        placeholder="e.g. Client work"
        class="mt-2"
        @keyup.enter="createGroupValid && confirmCreateGroup(() => (createGroupOpen = false))"
      />
      <p class="mt-1 text-xs text-muted-foreground">
        Only changes how the list is sorted. Move sites in from their ··· menu.
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
        sites aren't removed - they go back to their folder's section.
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
