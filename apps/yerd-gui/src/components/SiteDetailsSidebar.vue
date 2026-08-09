<script setup lang="ts">
import {
  ArrowUpRight,
  Code2,
  Copy,
  FileText,
  FolderOpen,
  Globe,
  Pencil,
  Terminal,
  Trash2,
  UserRound,
  X,
} from "lucide-vue-next";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";

import SiteDomainsPanel from "@/components/SiteDomainsPanel.vue";
import SiteRoutesPanel from "@/components/SiteRoutesPanel.vue";
import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import {
  IpcError,
  getInstalledIdes,
  openInBrowser,
  openInIde,
  openInSystemDefault,
  openInTerminal,
  openPath,
  pickDirectory,
  showDumpsWindow,
  wordpressAdminUsers,
} from "@/ipc/client";
import type { IdeOption, SiteEntry, StatusReport } from "@/ipc/types";
import { siteUrl } from "@/lib/siteUrl";
import { openWpAdmin } from "@/lib/wpAdmin";
import { useToast } from "@/composables/useToast";

const props = defineProps<{
  site: SiteEntry | null;
  open: boolean;
  report: StatusReport | null;
  tld: string;
  phpVersions: string[];
  /** Group picker options (including the "No group" entry). Empty when no
   *  groups are defined, which hides the row entirely. */
  groupOptions?: { value: string; label: string }[];
  /** The site's current group, or "" when ungrouped. */
  currentGroup?: string;
  busy?: boolean;
  /** Whether a "share publicly" action for this site is in flight. */
  sharing?: boolean;
}>();

const emit = defineEmits<{
  close: [];
  changePhp: [site: SiteEntry, version: string];
  changeWebRoot: [site: SiteEntry, path: string];
  toggleSecure: [site: SiteEntry];
  toggleFrontController: [site: SiteEntry, enabled: boolean];
  changeGroup: [site: SiteEntry, group: string];
  changeWpAutoLogin: [site: SiteEntry, enabled: boolean, user: string | null];
  share: [site: SiteEntry];
  unlink: [site: SiteEntry];
  domainsChanged: [];
}>();

const toast = useToast();
const activeTab = ref<"general" | "domains" | "routing" | "information">("general");
const webRoot = ref("");
const installedIdes = ref<IdeOption[]>([]);
const selectedIde = ref("auto");
let ideDetectionRequestId = 0;

const phpOptions = computed(() => {
  const versions = props.site
    ? Array.from(new Set([props.site.php, ...props.phpVersions]))
    : props.phpVersions;
  return versions.map((version) => ({ value: version, label: `PHP ${version}` }));
});

const ideOptions = computed(() => [
  { value: "auto", label: "Auto-detect" },
  ...installedIdes.value.map((ide) => ({ value: ide.id, label: ide.label })),
]);

const hasGroups = computed(() => (props.groupOptions?.length ?? 0) > 0);

const effectiveIde = computed(() => {
  if (selectedIde.value === "auto") return installedIdes.value[0]?.id ?? "system";
  if (selectedIde.value === "system") return "system";
  return installedIdes.value.some((ide) => ide.id === selectedIde.value)
    ? selectedIde.value
    : installedIdes.value[0]?.id ?? "system";
});

const editorLabel = computed(() => {
  if (effectiveIde.value === "system") return "Open folder";
  return installedIdes.value.find((ide) => ide.id === effectiveIde.value)?.label ?? "Editor";
});

const DEFAULT_ADMIN_OPTION = { value: "", label: "Earliest admin (default)" };
type WpAdminUsersStatus = "idle" | "loading" | "ready" | "error";
const wpAdminUsersStatus = ref<WpAdminUsersStatus>("idle");
const wpAdminUsersError = ref("");
const wpAdminUsersOptions = ref<{ value: string; label: string }[]>([DEFAULT_ADMIN_OPTION]);

/** The picker's real `:options` while loaded; a single synthetic "Loading…"/
 *  "Error: …" entry otherwise, paired with the Select's own `disabled` state
 *  (also gated on `wpAdminUsersStatus`) so an in-flight or failed fetch is
 *  never mistaken for "no other admins exist". */
const wpAdminUserSelectOptions = computed(() =>
  wpAdminUsersStatus.value === "loading"
    ? [{ value: "", label: "Loading users…" }]
    : wpAdminUsersStatus.value === "error"
      ? [{ value: "", label: `Error: ${wpAdminUsersError.value}` }]
      : wpAdminUsersOptions.value,
);

/** Bumped on every `loadWpAdminUsers` call so a response for a site the user
 *  has since navigated away from (closed the sidebar, opened another site's)
 *  can recognize it's stale and skip applying its result. */
let wpAdminUsersRequestId = 0;

async function loadWpAdminUsers(name: string): Promise<void> {
  const requestId = ++wpAdminUsersRequestId;
  wpAdminUsersStatus.value = "loading";
  try {
    const users = await wordpressAdminUsers(name);
    if (requestId !== wpAdminUsersRequestId) return;
    wpAdminUsersOptions.value = [
      DEFAULT_ADMIN_OPTION,
      ...users.map((u) => ({ value: u.login, label: u.display_name || u.login })),
    ];
    wpAdminUsersStatus.value = "ready";
  } catch (e) {
    if (requestId !== wpAdminUsersRequestId) return;
    wpAdminUsersError.value = (e as IpcError).message || "couldn't load admin users";
    wpAdminUsersStatus.value = "error";
  }
}

function displayHost(site: SiteEntry): string {
  return site.primary_domain ?? `${site.name}.${props.tld}`;
}

function servedLabel(site: SiteEntry): string {
  return site.web_subpath && site.web_subpath !== "" ? `/${site.web_subpath}` : "/";
}

function applicationLabel(site: SiteEntry): string {
  if (site.is_laravel) return "Laravel";
  if (site.is_wordpress) return "WordPress";
  return "PHP site";
}

async function openTerminal(site: SiteEntry): Promise<void> {
  try {
    await openInTerminal(site.document_root);
  } catch (error) {
    toast.error("Couldn't open terminal", (error as IpcError).message);
  }
}

async function openSite(site: SiteEntry, report: StatusReport | null): Promise<void> {
  try {
    await openInBrowser(siteUrl(site, report));
  } catch (error) {
    toast.error("Couldn't open site", (error as IpcError).message);
  }
}

async function revealSitePath(site: SiteEntry): Promise<void> {
  try {
    await openPath(site.document_root);
  } catch (error) {
    toast.error("Couldn't reveal site folder", (error as IpcError).message);
  }
}

async function loadInstalledIdes(): Promise<void> {
  const requestId = ++ideDetectionRequestId;
  try {
    const detected = await getInstalledIdes();
    if (requestId !== ideDetectionRequestId) return;
    installedIdes.value = detected;
  } catch (error) {
    if (requestId !== ideDetectionRequestId) return;
    installedIdes.value = [];
    toast.error("Couldn't detect installed IDEs", (error as IpcError).message);
  }
}

async function openEditor(site: SiteEntry): Promise<void> {
  try {
    const selectedIde = effectiveIde.value;
    if (selectedIde === "system") {
      await openInSystemDefault(site.document_root);
    } else {
      await openInIde(site.document_root, selectedIde);
    }
  } catch (error) {
    toast.error("Couldn't open the site folder", (error as IpcError).message);
  }
}

async function openDumps(): Promise<void> {
  try {
    await showDumpsWindow();
  } catch (error) {
    toast.error("Couldn't open the dumps window", (error as IpcError).message);
  }
}

function changeGroup(site: SiteEntry | null, group: string): void {
  if (site) emit("changeGroup", site, group);
}

function toggleWpAutoLogin(site: SiteEntry, enabled: boolean): void {
  emit("changeWpAutoLogin", site, enabled, site.wp_auto_login_user || null);
}

function changeWpAutoLoginUser(site: SiteEntry, user: string): void {
  emit("changeWpAutoLogin", site, true, user || null);
}

function changePhp(site: SiteEntry | null, version: string): void {
  if (site) emit("changePhp", site, version);
}

function changeWebRoot(site: SiteEntry | null): void {
  if (site) emit("changeWebRoot", site, webRoot.value.trim());
}

async function chooseWebRoot(site: SiteEntry): Promise<void> {
  try {
    const directory = await pickDirectory(site.document_root);
    if (!directory) return;
    const relative = relativeWebRoot(site.document_root, directory);
    if (relative === null) {
      toast.error("Invalid web root", "Choose a directory inside the site folder.");
      return;
    }
    webRoot.value = relative;
  } catch (error) {
    toast.error("Couldn't choose web root", (error as IpcError).message);
  }
}

function relativeWebRoot(siteRoot: string, selectedDirectory: string): string | null {
  const root = siteRoot.replace(/[\\/]+$/, "").replace(/[\\]/g, "/");
  const selected = selectedDirectory.replace(/[\\]/g, "/");
  if (root === "/") return selected.startsWith("/") ? selected.slice(1) : null;
  if (selected === root) return "";
  return selected.startsWith(`${root}/`) ? selected.slice(root.length + 1) : null;
}

async function copyWebRoot(): Promise<void> {
  try {
    await navigator.clipboard.writeText(webRoot.value);
    toast.success("Copied web root");
  } catch {
    toast.error("Couldn't copy web root", "Your browser blocked clipboard access.");
  }
}

const panel = ref<HTMLElement | null>(null);

/** Whether another modal dialog is layered above the sidebar. Dialogs opened
 *  from inside it (the unlink confirm) teleport into `<body>` after this panel,
 *  so a later `[role="dialog"]` in document order sits on top and owns Escape -
 *  without this both listeners fire and one keypress dismisses the pair. A panel
 *  that isn't in the document (a detached test mount) has index -1, so any other
 *  open dialog counts as being above it. */
function hasDialogAbove(): boolean {
  const dialogs = Array.from(document.querySelectorAll('[role="dialog"][aria-modal="true"]'));
  const own = panel.value ? dialogs.indexOf(panel.value) : -1;
  return dialogs.some((dialog, index) => dialog !== panel.value && index > own);
}

function onKeydown(event: KeyboardEvent): void {
  if (props.open && event.key === "Escape" && !hasDialogAbove()) emit("close");
}

// Resync the edit buffer from the site it belongs to. Keyed on the site name
// and the open flag as well as the saved value, because the parent keeps a
// single instance and swaps the `site` prop rather than remounting - two sites
// that both have no custom web root share the same `web_subpath`, so watching
// that alone would carry an unsaved edit across to the next site.
watch(
  [() => props.open, () => props.site?.name, () => props.site?.web_subpath],
  () => {
    webRoot.value = props.site?.web_subpath ?? "";
  },
  { immediate: true },
);

// Reopening (or retargeting) starts on General rather than whichever tab was
// left showing, and refetches the WordPress admin list for the site now in view.
// The fetch isn't gated on `wp_auto_login` being on: the picker has to be
// populated by the time the toggle is switched on, not after.
watch(
  [() => props.open, () => props.site?.name],
  () => {
    activeTab.value = "general";
    if (!props.open || !props.site) {
      ideDetectionRequestId += 1;
      return;
    }
    selectedIde.value = "auto";
    void loadInstalledIdes();
    if (!props.open || !props.site?.is_wordpress) return;
    wpAdminUsersStatus.value = "idle";
    wpAdminUsersOptions.value = [DEFAULT_ADMIN_OPTION];
    void loadWpAdminUsers(props.site.name);
  },
  { immediate: true },
);

onMounted(() => {
  document.addEventListener("keydown", onKeydown);
});
onUnmounted(() => {
  document.removeEventListener("keydown", onKeydown);
});
</script>

<template>
  <Teleport to="body">
    <Transition name="site-sidebar-backdrop">
      <div
        v-if="open && site"
        class="site-sidebar-backdrop fixed inset-0 z-40 bg-black/40"
        aria-hidden="true"
        @click="emit('close')"
      />
    </Transition>

    <Transition name="site-sidebar-panel">
      <aside
        v-if="open && site"
        ref="panel"
        class="fixed inset-y-0 right-0 z-50 flex w-full max-w-md flex-col border-l bg-background shadow-2xl"
        aria-label="Site details"
        role="dialog"
        aria-modal="true"
        @click.stop
      >
        <div class="flex items-start justify-between gap-4 border-b px-5 py-4">
          <div class="min-w-0">
            <div class="mb-2 flex items-center gap-2 text-muted-foreground">
              <Pencil class="size-4" />
              <span class="text-xs font-medium uppercase tracking-wider">Site details</span>
            </div>
            <h2 class="truncate font-mono text-lg font-medium">{{ displayHost(site) }}</h2>
            <p class="mt-1 text-xs text-muted-foreground">{{ applicationLabel(site) }}</p>
          </div>
          <Button variant="ghost" size="icon" aria-label="Close site details" @click="emit('close')">
            <X class="size-5" />
          </Button>
        </div>

        <div class="flex border-b px-5" role="tablist" aria-label="Site details sections">
          <button
            id="site-details-tab-general"
            type="button"
            class="border-b-2 px-3 py-2.5 text-xs font-medium transition-colors"
            :class="activeTab === 'general' ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground'"
            :aria-selected="activeTab === 'general'"
            aria-controls="site-details-panel-general"
            role="tab"
            @click="activeTab = 'general'"
          >
            General
          </button>
          <button
            id="site-details-tab-domains"
            type="button"
            class="border-b-2 px-3 py-2.5 text-xs font-medium transition-colors"
            :class="activeTab === 'domains' ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground'"
            :aria-selected="activeTab === 'domains'"
            aria-controls="site-details-panel-domains"
            role="tab"
            @click="activeTab = 'domains'"
          >
            Domains
          </button>
          <button
            id="site-details-tab-routing"
            type="button"
            class="border-b-2 px-3 py-2.5 text-xs font-medium transition-colors"
            :class="activeTab === 'routing' ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground'"
            :aria-selected="activeTab === 'routing'"
            aria-controls="site-details-panel-routing"
            role="tab"
            @click="activeTab = 'routing'"
          >
            Routing
          </button>
          <button
            id="site-details-tab-information"
            type="button"
            class="border-b-2 px-3 py-2.5 text-xs font-medium transition-colors"
            :class="activeTab === 'information' ? 'border-primary text-foreground' : 'border-transparent text-muted-foreground hover:text-foreground'"
            :aria-selected="activeTab === 'information'"
            aria-controls="site-details-panel-information"
            role="tab"
            @click="activeTab = 'information'"
          >
            Information
          </button>
        </div>

        <div
          :id="`site-details-panel-${activeTab}`"
          class="min-h-0 flex-1 overflow-y-auto px-5 py-5"
          role="tabpanel"
          :aria-labelledby="`site-details-tab-${activeTab}`"
        >
          <template v-if="activeTab === 'general'">
            <Button class="w-full" @click="openSite(site, report)">
              Open site
              <ArrowUpRight />
            </Button>

            <div class="mt-4 grid grid-cols-2 gap-2">
              <Button class="min-w-0 px-2" variant="outline" size="sm" @click="openTerminal(site)">
                <Terminal /> <span class="truncate">Terminal</span>
              </Button>
              <Button
                class="min-w-0 px-2"
                variant="outline"
                size="sm"
                :title="`Open the site folder in ${editorLabel}`"
                @click="openEditor(site)"
              >
                <Code2 /> <span class="truncate">{{ editorLabel }}</span>
              </Button>
              <Button
                class="min-w-0 px-2"
                variant="outline"
                size="sm"
                title="Open the Dumps viewer (captured dump/query telemetry, all sites)"
                @click="openDumps"
              >
                <FileText /> <span class="truncate">Dumps</span>
              </Button>
              <Button
                v-if="site.is_wordpress"
                class="min-w-0 px-2"
                variant="outline"
                size="sm"
                title="Signs you in automatically when auto-login is enabled"
                @click="openWpAdmin(site, report)"
              >
                <UserRound /> <span class="truncate">WP Admin</span>
              </Button>
              <Button
                class="min-w-0 px-2"
                variant="outline"
                size="sm"
                :disabled="sharing"
                title="Publish this site over a Cloudflare Quick Tunnel"
                @click="emit('share', site)"
              >
                <Spinner v-if="sharing" class="size-4" />
                <Globe v-else />
                <span class="truncate">Share publicly…</span>
              </Button>
            </div>

            <dl class="mt-5 divide-y rounded-lg border">
              <div class="flex items-center justify-between gap-4 px-3 py-3">
                <dt class="shrink-0 text-xs text-muted-foreground">PHP version</dt>
                <dd class="min-w-0">
                  <Select
                    :model-value="site.php"
                    :options="phpOptions"
                    aria-label="Site PHP version"
                    :disabled="busy || phpOptions.length === 0"
                    @update:model-value="changePhp(site, $event)"
                  />
                </dd>
              </div>
              <div class="flex items-center justify-between gap-4 px-3 py-3">
                <dt class="shrink-0 text-xs text-muted-foreground">IDE switch</dt>
                <dd class="min-w-0">
                  <Select
                    :model-value="selectedIde"
                    :options="ideOptions"
                    aria-label="Site IDE"
                    :disabled="busy"
                    @update:model-value="selectedIde = $event"
                  />
                </dd>
              </div>
              <div class="px-3 py-3">
                <dt class="text-xs text-muted-foreground">Path</dt>
                <dd class="mt-1 flex items-start gap-2 text-sm">
                  <FolderOpen class="mt-0.5 size-4 shrink-0 text-muted-foreground" />
                  <button
                    class="min-w-0 break-all text-left font-mono hover:text-brand"
                    :title="`Reveal ${site.document_root}`"
                    @click="revealSitePath(site)"
                  >
                    {{ site.document_root }}
                  </button>
                </dd>
              </div>
              <div class="px-3 py-3">
                <label class="block text-xs text-muted-foreground" for="site-web-root">Web root</label>
                <div class="mt-1.5 flex gap-2">
                  <Input
                    id="site-web-root"
                    v-model="webRoot"
                    class="h-8 min-w-0"
                    :disabled="busy"
                    placeholder="public"
                    aria-label="Site web root"
                    @keydown.enter="changeWebRoot(site)"
                  />
                  <Button
                    variant="outline"
                    :disabled="busy"
                    aria-label="Choose web root directory"
                    @click="chooseWebRoot(site)"
                  >
                    <FolderOpen class="size-4" />
                  </Button>
                  <Button
                    variant="outline"
                    :disabled="busy"
                    aria-label="Copy web root"
                    title="Copy web root"
                    @click="copyWebRoot"
                  >
                    <Copy class="size-4" />
                  </Button>
                </div>
                <p class="mt-1 text-xs text-muted-foreground">
                  Directory served as the document root, relative to the site folder (e.g.
                  <code class="font-mono">public</code>). Leave blank to auto-detect.
                </p>
                <div v-if="webRoot.trim() !== (site.web_subpath ?? '')" class="mt-2 flex justify-end">
                  <Button
                    variant="outline"
                    size="sm"
                    :disabled="busy"
                    @click="changeWebRoot(site)"
                  >
                    Save
                  </Button>
                </div>
              </div>
              <div class="px-3 py-3">
                <dt class="text-xs text-muted-foreground">URL</dt>
                <dd class="mt-1 break-all font-mono text-sm">{{ siteUrl(site, report) }}</dd>
              </div>
              <div class="px-3 py-3">
                <div class="flex items-center justify-between gap-4">
                  <div>
                    <dt class="text-sm font-medium">HTTPS</dt>
                    <dd class="mt-1 text-xs text-muted-foreground">Serve this site over TLS.</dd>
                  </div>
                  <Switch
                    :model-value="site.secure"
                    :disabled="busy"
                    aria-label="HTTPS"
                    @update:model-value="emit('toggleSecure', site)"
                  />
                </div>
              </div>
              <div v-if="!site.is_wordpress && site.uses_front_controller !== undefined" class="px-3 py-3">
                <div class="flex items-center justify-between gap-4">
                  <div>
                    <dt class="text-sm font-medium">Route through front controller</dt>
                    <dd class="mt-1 text-xs text-muted-foreground">
                      On: every request funnels through the site's <code class="font-mono">index.php</code>
                      (Laravel, Symfony). Off: named <code class="font-mono">.php</code> files are executed directly
                      (plain PHP).
                    </dd>
                  </div>
                  <Switch
                    :model-value="site.uses_front_controller"
                    :disabled="busy"
                    aria-label="Route through front controller"
                    @update:model-value="emit('toggleFrontController', site, $event)"
                  />
                </div>
              </div>
              <div v-if="site.is_wordpress" class="px-3 py-3">
                <div class="flex items-center justify-between gap-4">
                  <div>
                    <dt class="text-sm font-medium">WordPress Auto Admin Login</dt>
                    <dd class="mt-1 text-xs text-muted-foreground">
                      Sign in automatically when opening WP Admin.
                    </dd>
                  </div>
                  <Switch
                    :model-value="site.wp_auto_login ?? false"
                    :disabled="busy"
                    aria-label="WordPress Auto Admin Login"
                    @update:model-value="toggleWpAutoLogin(site, $event)"
                  />
                </div>
                <div v-if="site.wp_auto_login" class="mt-3">
                  <label class="block text-xs text-muted-foreground" for="site-wp-admin-user">
                    Sign in as
                  </label>
                  <div class="mt-1.5">
                    <Select
                      id="site-wp-admin-user"
                      :model-value="wpAdminUsersStatus === 'ready' ? (site.wp_auto_login_user ?? '') : ''"
                      :options="wpAdminUserSelectOptions"
                      :disabled="busy || wpAdminUsersStatus !== 'ready'"
                      class="w-full"
                      aria-label="Sign in as"
                      @update:model-value="changeWpAutoLoginUser(site, $event)"
                    />
                  </div>
                </div>
              </div>
              <div v-if="hasGroups" class="px-3 py-3">
                <div class="flex items-center justify-between gap-4">
                  <dt class="shrink-0 text-xs text-muted-foreground">Group</dt>
                  <dd class="min-w-0">
                    <Select
                      :model-value="currentGroup ?? ''"
                      :options="groupOptions ?? []"
                      :disabled="busy"
                      aria-label="Site group"
                      @update:model-value="changeGroup(site, $event)"
                    />
                  </dd>
                </div>
              </div>
            </dl>

            <!-- Only linked sites are removable here (by name). A parked site is
                 removed by un-parking its folder. -->
            <div v-if="site.kind === 'linked'" class="mt-5 rounded-lg border border-destructive/40 p-3">
              <div class="flex items-center justify-between gap-4">
                <div class="min-w-0">
                  <p class="text-sm font-medium">Unlink this site</p>
                  <p class="mt-1 text-xs text-muted-foreground">
                    Stops serving {{ displayHost(site) }}. The project folder is left untouched.
                  </p>
                </div>
                <Button
                  variant="outline"
                  size="sm"
                  class="shrink-0 border-destructive/40 text-destructive hover:bg-destructive/10 hover:text-destructive"
                  :disabled="busy"
                  @click="emit('unlink', site)"
                >
                  <Trash2 class="size-4" /> Unlink
                </Button>
              </div>
            </div>
          </template>

          <template v-else-if="activeTab === 'domains'">
            <SiteDomainsPanel :site="site" :tld="tld" @changed="emit('domainsChanged')" />
          </template>

          <template v-else-if="activeTab === 'routing'">
            <SiteRoutesPanel :site="site" />
          </template>

          <template v-else>
            <dl class="divide-y rounded-lg border">
              <div class="px-3 py-3">
                <dt class="text-xs text-muted-foreground">Application</dt>
                <dd class="mt-1 text-sm">{{ applicationLabel(site) }}</dd>
              </div>
              <div class="px-3 py-3">
                <dt class="text-xs text-muted-foreground">Domains</dt>
                <dd class="mt-1 space-y-1 font-mono text-sm">
                  <div v-for="domain in site.domains ?? [displayHost(site)]" :key="domain">
                    {{ domain }}
                  </div>
                </dd>
              </div>
              <div class="grid grid-cols-2 divide-x border-t">
                <div class="px-3 py-3">
                  <dt class="text-xs text-muted-foreground">Web root</dt>
                  <dd class="mt-1 font-mono text-sm">{{ servedLabel(site) }}</dd>
                </div>
                <div class="px-3 py-3 pl-4">
                  <dt class="text-xs text-muted-foreground">Type</dt>
                  <dd class="mt-1 capitalize text-sm">{{ site.kind }}</dd>
                </div>
              </div>
              <div class="grid grid-cols-2 divide-x border-t">
                <div class="px-3 py-3">
                  <dt class="text-xs text-muted-foreground">PHP</dt>
                  <dd class="mt-1 font-mono text-sm">{{ site.php }}</dd>
                </div>
                <div class="px-3 py-3 pl-4">
                  <dt class="text-xs text-muted-foreground">Protocol</dt>
                  <dd class="mt-1 text-sm">{{ site.secure ? "HTTPS" : "HTTP" }}</dd>
                </div>
              </div>
              <div v-if="site.uses_front_controller !== undefined" class="px-3 py-3">
                <dt class="text-xs text-muted-foreground">Front controller</dt>
                <dd class="mt-1 text-sm">{{ site.uses_front_controller ? "Enabled" : "Disabled" }}</dd>
              </div>
              <div v-if="site.is_wordpress" class="px-3 py-3">
                <dt class="text-xs text-muted-foreground">WordPress auto-login</dt>
                <dd class="mt-1 text-sm">
                  {{ site.wp_auto_login ? `Enabled${site.wp_auto_login_user ? ` as ${site.wp_auto_login_user}` : ""}` : "Disabled" }}
                </dd>
              </div>
            </dl>

            <p v-if="site.apex_shadowed_by" class="mt-4 rounded-md border border-warning/40 bg-warning/10 p-3 text-xs text-warning">
              {{ site.name }}.{{ tld }} is served by “{{ site.apex_shadowed_by }}”.
            </p>
          </template>
        </div>
      </aside>
    </Transition>
  </Teleport>
</template>

<style scoped>
.site-sidebar-backdrop-enter-active,
.site-sidebar-backdrop-leave-active {
  transition: opacity 180ms ease;
}

.site-sidebar-backdrop-enter-from,
.site-sidebar-backdrop-leave-to {
  opacity: 0;
}

.site-sidebar-panel-enter-active,
.site-sidebar-panel-leave-active {
  transition: transform 220ms ease;
}

.site-sidebar-panel-enter-from,
.site-sidebar-panel-leave-to {
  transform: translateX(100%);
}
</style>
