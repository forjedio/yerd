<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref, watch } from "vue";
import {
  CheckCircle2,
  Cloud,
  Copy,
  Download,
  Globe,
  Lock,
  Share2,
  Square,
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
import Combobox from "@/components/ui/Combobox.vue";
import EmptyState from "@/components/ui/EmptyState.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import {
  cloudflaredLogin,
  createNamedTunnel,
  deleteNamedTunnel,
  installCloudflaredStreamed,
  IpcError,
  listNamedTunnels,
  listSites,
  openInBrowser,
  pollJobToEnd,
  routeTunnelDns,
  setSiteTunnel,
  startNamedTunnel,
  startQuickTunnel,
  stopNamedTunnel,
  stopTunnel,
  tunnelStatus,
} from "@/ipc/client";
import type { CloudflaredStatus, NamedTunnelMeta, Site, TunnelInfo } from "@/ipc/types";

const toast = useToast();
const { connected } = useDaemon();

const cloudflared = ref<CloudflaredStatus | null>(null);
const tunnels = ref<TunnelInfo[]>([]);
const sites = ref<Site[]>([]);
const shareSite = ref("");
// A long-running op in flight, e.g. "share:app" / "stop:app" / "install".
const busy = ref<string | null>(null);

const installed = computed(() => cloudflared.value?.installed ?? false);
const loggedIn = computed(() => cloudflared.value?.logged_in ?? false);

// Named tunnels (Phase 2). One consolidated tunnel exposes every enabled site.
const namedTunnels = ref<NamedTunnelMeta[]>([]);
const newTunnelName = ref("");
// site -> hostname currently enabled on the server.
const enabledHosts = ref<Record<string, string>>({});
// site -> hostname being typed in the per-site input.
const hostInputs = ref<Record<string, string>>({});

const namedRunning = computed(() =>
  tunnels.value.some((t) => t.kind === "named" && t.state === "running"),
);
// Whether any site is currently exposed (drives auto start/stop of the tunnel).
const anyExposed = computed(() => Object.keys(enabledHosts.value).length > 0);
// v1 allows exactly one named tunnel.
const hasNamedTunnel = computed(() => namedTunnels.value.length > 0);
// The authorized Cloudflare zone (domain), once resolved from the account cert.
const authorizedDomain = ref<string | null>(null);

// Site search for the expose table.
const siteFilter = ref("");
// Sites for the expose table: matching the filter, exposed ones first.
const visibleSites = computed(() => {
  const q = siteFilter.value.trim().toLowerCase();
  const matched = q ? sites.value.filter((s) => s.name.toLowerCase().includes(q)) : sites.value;
  return [...matched].sort((a, b) => {
    const ax = enabledHosts.value[a.name] ? 0 : 1;
    const bx = enabledHosts.value[b.name] ? 0 : 1;
    return ax - bx || a.name.localeCompare(b.name);
  });
});

/** Suggested public hostname for a site: `{site}.{domain}` once the domain is
 *  known, otherwise a generic placeholder. */
function suggestedHost(site: string): string {
  return authorizedDomain.value ? `${site}.${authorizedDomain.value}` : `${site}.example.com`;
}
// The per-site "Shared sites" table is Quick-only; the named tunnel is managed
// in its own card.
const quickTunnels = computed(() => tunnels.value.filter((t) => t.kind === "quick"));

// Sites not already tunnelled, for the share picker.
const shareableSites = computed(() => {
  const live = new Set(tunnels.value.map((t) => t.site));
  return sites.value.filter((s) => !live.has(s.name));
});

const shareOptions = computed(() =>
  shareableSites.value.map((s) => ({
    value: s.name,
    label: s.name,
    sublabel: `PHP ${s.php}${s.secure ? " · https" : ""}`,
  })),
);

const selectedShareSite = computed(() =>
  sites.value.find((s) => s.name === shareSite.value),
);

// Streamed install log.
const logOpen = ref(false);
const installLog = ref<string[]>([]);
const logBox = ref<HTMLElement | null>(null);

async function appendLog(lines: string[]): Promise<void> {
  installLog.value.push(...lines);
  await nextTick();
  const el = logBox.value;
  if (el) el.scrollTop = el.scrollHeight;
}

type SharePageData = {
  cloudflared: CloudflaredStatus;
  tunnels: TunnelInfo[];
  sites: Site[];
  named: Awaited<ReturnType<typeof listNamedTunnels>>;
};

// The named-tunnel list is fetched unconditionally: the daemon returns the saved
// tunnels and site mappings even when logged out (only the live zone needs an
// account), so a logged-out user keeps visibility and control of an existing
// tunnel instead of having it vanish from the UI.
async function fetchSharePage(): Promise<SharePageData> {
  const [status, siteList, named] = await Promise.all([
    tunnelStatus(),
    listSites(),
    listNamedTunnels(),
  ]);
  return { cloudflared: status.cloudflared, tunnels: status.tunnels, sites: siteList, named };
}

// Module-cached (stale-while-revalidate): revisiting the page renders the last
// value instantly and revalidates in the background, instead of re-fetching and
// flashing a spinner on every pageload.
const { data, loading, refresh, mutate, error } = useResource("share-page", fetchSharePage);

// Fold an authoritative tunnel-status response into the cache. Writing through
// `mutate` (not the local refs directly) bumps useResource's epoch, so a
// background poll fetch that started before this action can't revert it.
function applyTunnelStatus(r: { tunnels: TunnelInfo[]; cloudflared: CloudflaredStatus }): void {
  mutate((cur) => (cur ? { ...cur, tunnels: r.tunnels, cloudflared: r.cloudflared } : cur));
}

// Optimistically add/clear a site's hostname mapping in the cache.
function setNamedSite(site: string, hostname: string | null): void {
  mutate((cur) => {
    if (!cur?.named) return cur;
    const sites = cur.named.sites.filter((s) => s.site !== site);
    if (hostname) sites.push({ site, hostname });
    return { ...cur, named: { ...cur.named, sites } };
  });
}

/** Mirror fetched page data into the local refs the handlers mutate optimistically. */
function applyData(d: SharePageData): void {
  cloudflared.value = d.cloudflared;
  tunnels.value = d.tunnels;
  sites.value = d.sites;
  const staleSelection =
    shareSite.value && !shareableSites.value.some((s) => s.name === shareSite.value);
  if (staleSelection) {
    shareSite.value = "";
  }
  namedTunnels.value = d.named.tunnels;
  authorizedDomain.value = d.named.zone ?? null;
  enabledHosts.value = Object.fromEntries(d.named.sites.map((s) => [s.site, s.hostname]));
  for (const s of d.sites) {
    if (hostInputs.value[s.name] === undefined || hostInputs.value[s.name] === "") {
      hostInputs.value[s.name] = enabledHosts.value[s.name] ?? suggestedHost(s.name);
    }
  }
}

watch(data, (d) => { if (d) applyData(d); }, { immediate: true });
// Surface a load failure only on a cold load (no cached data), so a transient
// background revalidation stays silent.
watch(error, (e) => {
  if (e && !data.value) toast.error("Couldn't load tunnels", e.message);
});

/** Force a revalidation and apply it synchronously (for post-write refreshes). */
async function reload(): Promise<void> {
  await refresh({ force: true });
  if (data.value) applyData(data.value);
}

// ── named tunnels (Phase 2) ───────────────────────────────────────────────

const loginOpen = ref(false);
const loginLog = ref<string[]>([]);
const loginBox = ref<HTMLElement | null>(null);
// The auth URL, captured once so we can offer a manual "Open" button. cloudflared
// opens the browser itself, so we deliberately do NOT auto-open it here (doing so
// produced duplicate browser windows).
const loginUrl = ref<string | null>(null);

async function appendLogin(lines: string[]): Promise<void> {
  for (const line of lines) {
    const m = line.match(/https:\/\/dash\.cloudflare\.com\/argotunnel\S*/);
    if (m && !loginUrl.value) loginUrl.value = m[0];
  }
  loginLog.value.push(...lines);
  await nextTick();
  const el = loginBox.value;
  if (el) el.scrollTop = el.scrollHeight;
}

async function doLogin(): Promise<void> {
  busy.value = "login";
  loginLog.value = [];
  loginUrl.value = null;
  loginOpen.value = true;
  try {
    const jobId = await cloudflaredLogin();
    const final = await pollJobToEnd(jobId, (lines) => void appendLogin(lines), () => loginOpen.value);
    await reload();
    if (final.state === "succeeded") {
      toast.success("Connected to Cloudflare");
    } else if (final.state !== "running") {
      toast.error("Cloudflare login failed", final.error ?? "login failed");
    }
  } catch (e) {
    toast.error("Cloudflare login failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doCreateTunnel(): Promise<void> {
  const name = newTunnelName.value.trim();
  if (!name) return;
  busy.value = "create";
  try {
    await createNamedTunnel(name);
    newTunnelName.value = "";
    await reload();
    toast.success(`Created tunnel ${name}`);
  } catch (e) {
    toast.error(`Couldn't create ${name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── delete-tunnel confirm ──
const deleteTunnelOpen = ref(false);
const deleteTunnelTarget = ref<string | null>(null);

function askDeleteTunnel(name: string): void {
  deleteTunnelTarget.value = name;
  deleteTunnelOpen.value = true;
}

/** Delete the named tunnel from the account and reset local state. */
async function confirmDeleteTunnel(close: () => void): Promise<void> {
  const name = deleteTunnelTarget.value;
  if (!name) return;
  close();
  busy.value = "delete-tunnel";
  try {
    await deleteNamedTunnel(name);
    await reload();
    toast.success(`Deleted tunnel ${name}`);
  } catch (e) {
    toast.error(`Couldn't delete ${name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
    deleteTunnelTarget.value = null;
  }
}

/** Enable a site: save its hostname and route DNS to the named tunnel. */
async function doEnableSite(site: string): Promise<void> {
  const hostname = (hostInputs.value[site] ?? "").trim();
  const tunnel = namedTunnels.value[0];
  if (!hostname || !tunnel) return;
  busy.value = `enable:${site}`;
  try {
    await setSiteTunnel(site, hostname);
    try {
      await routeTunnelDns(tunnel.name, hostname);
    } catch (routeErr) {
      const rolledBack = await setSiteTunnel(site, null)
        .then(() => true)
        .catch(() => false);
      if (!rolledBack) await reload();
      throw routeErr;
    }
    setNamedSite(site, hostname);
    await reconcileNamedTunnel(true);
    toast.success(`Exposed ${site}`, `https://${hostname}`);
  } catch (e) {
    toast.error(`Couldn't expose ${site}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

/** Disable a site: clear its hostname mapping. */
async function doDisableSite(site: string): Promise<void> {
  busy.value = `enable:${site}`;
  try {
    await setSiteTunnel(site, null);
    const stillExposed = Object.keys(enabledHosts.value).some((s) => s !== site);
    setNamedSite(site, null);
    await reconcileNamedTunnel(stillExposed);
    toast.success(`Removed ${site} from the tunnel`);
  } catch (e) {
    toast.error(`Couldn't remove ${site}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

/** Keep the live tunnel in sync with the exposed set: (re)start it while a site
 *  is exposed, stop it once the last one is removed. `hasExposed` is supplied by
 *  the caller (the cache mutation is async via the watcher, so we don't read it
 *  back here). */
async function reconcileNamedTunnel(hasExposed: boolean): Promise<void> {
  const r = hasExposed ? await startNamedTunnel() : await stopNamedTunnel();
  applyTunnelStatus(r);
}

/** Manual restart, for recovering a tunnel that died (state "failed"). */
async function doRestartNamed(): Promise<void> {
  busy.value = "named";
  const hasExposed = anyExposed.value;
  try {
    await reconcileNamedTunnel(hasExposed);
    toast.success(hasExposed ? "Tunnel restarted" : "Tunnel stopped");
  } catch (e) {
    toast.error("Couldn't restart the tunnel", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doInstall(): Promise<void> {
  busy.value = "install";
  installLog.value = [];
  logOpen.value = true;
  try {
    const jobId = await installCloudflaredStreamed();
    const final = await pollJobToEnd(jobId, (lines) => void appendLog(lines), () => logOpen.value);
    await reload();
    if (final.state === "succeeded") {
      toast.success("Installed cloudflared");
    } else if (final.state !== "running") {
      toast.error("cloudflared install failed", final.error ?? "install failed");
    }
  } catch (e) {
    toast.error("cloudflared install failed", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doShare(): Promise<void> {
  const site = shareSite.value;
  if (!site) return;
  busy.value = `share:${site}`;
  try {
    const r = await startQuickTunnel(site);
    applyTunnelStatus(r);
    shareSite.value = "";
    const url = r.tunnels.find((t) => t.site === site)?.url;
    toast.success(`Sharing ${site}`, url ?? undefined);
  } catch (e) {
    toast.error(`Couldn't share ${site}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doStop(site: string): Promise<void> {
  busy.value = `stop:${site}`;
  try {
    const r = await stopTunnel(site);
    applyTunnelStatus(r);
    toast.success(`Stopped sharing ${site}`);
  } catch (e) {
    toast.error(`Couldn't stop ${site}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function copyUrl(url: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(url);
    toast.success("Copied URL");
  } catch {
    toast.error("Couldn't copy URL");
  }
}

// Open an external URL through the Tauri shell opener, surfacing (not floating)
// any failure.
function openExternal(url: string): void {
  void openInBrowser(url).catch(() => toast.error("Couldn't open link"));
}

// Silent background revalidation while a tunnel is live, so a cloudflared child
// that dies (state -> "failed") becomes visible without a manual reload. Skipped
// while a long-running op is in flight to avoid clobbering optimistic UI; cleared
// on unmount. useResource fetches on mount and serves the cached value instantly
// on a revisit, so there's no explicit load() here.
let poll: ReturnType<typeof setInterval> | null = null;
async function refreshTunnels(): Promise<void> {
  if (busy.value !== null || tunnels.value.length === 0) return;
  await reload().catch(() => {});
}

onMounted(() => {
  poll = setInterval(() => void refreshTunnels(), 5000);
});
onUnmounted(() => {
  if (poll !== null) clearInterval(poll);
  logOpen.value = false;
  loginOpen.value = false;
});
onUnmounted(registerViewActions({ refresh: () => void reload() }));
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Share"
      subtitle="Publish a local site to the internet over a secure public URL"
    />

    <div class="flex-1 space-y-6 overflow-y-auto p-6">
      <Card>
        <CardHeader>
          <CardTitle>Cloudflare Tunnel</CardTitle>
          <CardDescription>
            Expose a local site over a public HTTPS URL through Cloudflare's
            edge - outbound-only, no open ports, no account needed. Quick
            tunnels are for development: the URL is temporary (it changes on
            restart), capped at 200 concurrent requests, and does not support
            server-sent events.
          </CardDescription>
        </CardHeader>

        <CardContent>
          <div v-if="loading" class="flex justify-center py-8">
            <Spinner class="size-6" />
          </div>

          <div v-else class="flex items-center justify-between gap-4">
            <div class="flex items-center gap-2 text-sm">
              <Badge v-if="installed" variant="secondary" class="gap-1">
                <CheckCircle2 class="size-3 text-emerald-500" />
                cloudflared {{ cloudflared?.version ?? "ready" }}
              </Badge>
              <span v-else class="text-muted-foreground">
                Sharing needs <span class="font-medium text-foreground">cloudflared</span>,
                a small one-time download. No account required.
              </span>
            </div>
            <Button
              v-if="!installed"
              :disabled="busy !== null || connected !== true"
              @click="doInstall"
            >
              <Download class="mr-1.5 size-3.5" /> Install cloudflared
            </Button>
          </div>
        </CardContent>
      </Card>

      <Card v-if="installed">
        <CardHeader>
          <CardTitle>Shared sites</CardTitle>
          <CardDescription>
            Pick a site to publish, or stop an active tunnel. Sharing serves
            your local site as-is - anyone with the URL can reach it.
          </CardDescription>
        </CardHeader>

        <CardContent>
          <div class="mb-4 flex items-end gap-3">
            <div class="min-w-0 flex-1">
              <label class="mb-1.5 block text-xs font-medium text-muted-foreground">
                Site to share
              </label>
              <Combobox
                v-model="shareSite"
                :options="shareOptions"
                placeholder="Choose a site…"
                search-placeholder="Search sites…"
                empty-text="No matching sites."
                aria-label="Site to share"
                :disabled="shareableSites.length === 0"
              />
            </div>
            <Button
              :disabled="busy !== null || !shareSite || connected !== true"
              @click="doShare"
            >
              <Spinner v-if="busy === `share:${shareSite}`" class="mr-1.5 size-3.5" />
              <Globe v-else class="mr-1.5 size-3.5" />
              Share
            </Button>
          </div>

          <p
            v-if="selectedShareSite"
            class="mb-5 flex items-center gap-1.5 text-xs text-muted-foreground"
          >
            <Lock v-if="selectedShareSite.secure" class="size-3 shrink-0" />
            <span>
              Publishes
              <span class="font-mono text-foreground">{{ shareSite }}.test</span>
              over {{ selectedShareSite.secure ? "HTTPS" : "HTTP" }} - anyone with the
              link can reach it.
            </span>
          </p>

          <EmptyState
            v-if="quickTunnels.length === 0"
            :icon="Share2"
            title="No active shares"
            description="Pick a site above and hit Share to get a public URL you can send to anyone."
          />

          <table v-else class="w-full text-sm">
            <thead>
              <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                <th class="py-2 pr-4 font-medium">Site</th>
                <th class="py-2 pr-4 font-medium">Public URL</th>
                <th class="py-2 pl-4 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="t in quickTunnels" :key="t.site" class="border-b last:border-0">
                <td class="py-3 pr-4 font-medium text-foreground">
                  {{ t.site }}
                  <Badge v-if="t.state === 'failed'" variant="outline" class="ml-1">
                    failed
                  </Badge>
                </td>
                <td class="py-3 pr-4">
                  <button
                    v-if="t.url ?? t.hostname"
                    type="button"
                    class="font-mono text-xs text-brand hover:underline"
                    @click="openExternal(t.url ?? `https://${t.hostname}`)"
                  >
                    {{ t.url ?? t.hostname }}
                  </button>
                  <span v-else class="text-xs text-muted-foreground">starting…</span>
                </td>
                <td class="py-3 pl-4">
                  <div class="flex items-center justify-end gap-2">
                    <Button
                      v-if="t.url"
                      variant="ghost"
                      size="sm"
                      aria-label="Copy URL"
                      title="Copy URL"
                      @click="copyUrl(t.url)"
                    >
                      <Copy class="size-3.5" />
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      :disabled="busy !== null"
                      @click="doStop(t.site)"
                    >
                      <Spinner v-if="busy === `stop:${t.site}`" class="mr-1.5 size-3.5" />
                      <Square v-else class="mr-1.5 size-3.5" />
                      Stop
                    </Button>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </CardContent>
      </Card>

      <Card v-if="installed">
        <CardHeader>
          <CardTitle>Named tunnels (stable hostnames)</CardTitle>
          <CardDescription>
            Publish a site at a stable hostname on your own Cloudflare domain
            (requires a Cloudflare account and a managed domain). Yerd uses one
            tunnel to serve every exposed site.
          </CardDescription>
        </CardHeader>

        <CardContent>
          <div
            v-if="!loggedIn"
            class="mb-5 flex items-center justify-between gap-4 rounded-md border bg-muted/40 px-3 py-2.5"
          >
            <span class="text-sm text-muted-foreground">
              {{ hasNamedTunnel
                ? "Reconnect your Cloudflare account to manage this tunnel."
                : "Not connected to a Cloudflare account." }}
            </span>
            <Button :disabled="busy !== null || connected !== true" @click="doLogin">
              <Cloud class="mr-1.5 size-3.5" /> Connect Cloudflare account
            </Button>
          </div>

          <div v-if="loggedIn || hasNamedTunnel" class="space-y-5">
            <div class="flex items-center justify-between gap-2">
              <Badge v-if="loggedIn" variant="secondary">
                Cloudflare connected<template v-if="authorizedDomain">
                  · {{ authorizedDomain }}</template>
              </Badge>
              <Badge v-else variant="outline" class="text-muted-foreground">
                Account disconnected
              </Badge>
              <div class="flex items-center gap-2">
                <Badge v-if="namedRunning" variant="secondary" class="gap-1">
                  <span class="size-1.5 rounded-full bg-emerald-500" /> live
                </Badge>
                <Badge
                  v-else-if="anyExposed"
                  variant="outline"
                  class="gap-1 text-amber-600 dark:text-amber-400"
                >
                  offline
                </Badge>
                <Button
                  v-if="anyExposed"
                  variant="outline"
                  size="sm"
                  :disabled="busy !== null || connected !== true || !loggedIn"
                  @click="doRestartNamed"
                >
                  <Spinner v-if="busy === 'named'" class="mr-1.5 size-3.5" />
                  <Globe v-else class="mr-1.5 size-3.5" />
                  {{ namedRunning ? "Restart" : "Start" }}
                </Button>
              </div>
            </div>

            <div>
              <div v-if="!hasNamedTunnel">
                <label class="mb-1 block text-xs text-muted-foreground">Create a tunnel</label>
                <div class="flex items-end gap-2">
                  <Input
                    v-model="newTunnelName"
                    placeholder="tunnel name"
                    class="flex-1"
                  />
                  <Button
                    :disabled="busy !== null || !newTunnelName.trim() || connected !== true || !loggedIn"
                    @click="doCreateTunnel"
                  >
                    <Spinner v-if="busy === 'create'" class="mr-1.5 size-3.5" />
                    Create
                  </Button>
                </div>
              </div>
              <ul v-if="namedTunnels.length" class="mt-2 space-y-1.5">
                <li
                  v-for="t in namedTunnels"
                  :key="t.uuid"
                  class="flex items-center justify-between gap-2 rounded-md border bg-muted/40 px-2.5 py-1.5"
                >
                  <div class="min-w-0">
                    <p class="truncate text-sm font-medium">{{ t.name }}</p>
                    <p class="truncate font-mono text-[11px] text-muted-foreground">{{ t.uuid }}</p>
                  </div>
                  <Button
                    variant="ghost"
                    size="sm"
                    class="shrink-0 text-destructive hover:bg-destructive/10 hover:text-destructive"
                    :disabled="busy !== null || connected !== true || !loggedIn"
                    @click="askDeleteTunnel(t.name)"
                  >
                    <Spinner v-if="busy === 'delete-tunnel'" class="mr-1.5 size-3.5" />
                    <Trash2 v-else class="mr-1.5 size-3.5" />
                    Remove
                  </Button>
                </li>
              </ul>
            </div>

            <div>
              <h3 class="mb-2 text-sm font-semibold text-foreground">
                Choose a site to expose
              </h3>

              <p v-if="namedTunnels.length === 0" class="text-xs text-muted-foreground">
                Create a tunnel above first.
              </p>
              <p v-else-if="sites.length === 0" class="text-xs text-muted-foreground">
                No sites yet.
              </p>

              <template v-else>
                <div class="mb-2">
                  <Input v-model="siteFilter" placeholder="Search sites…" />
                </div>
                <p
                  v-if="visibleSites.length === 0"
                  class="py-4 text-center text-xs text-muted-foreground"
                >
                  No sites match “{{ siteFilter }}”.
                </p>
                <table v-else class="w-full text-sm">
                  <thead>
                    <tr class="border-b text-left text-xs uppercase text-muted-foreground">
                      <th class="py-2 pr-2 font-medium">Site</th>
                      <th class="py-2 pr-2 font-medium">Public hostname</th>
                      <th class="py-2 pl-2 text-right font-medium">Action</th>
                    </tr>
                  </thead>
                  <tbody>
                    <tr v-for="s in visibleSites" :key="s.name" class="border-b last:border-0">
                      <td class="py-2 pr-2 font-medium text-foreground">
                        {{ s.name }}
                        <Badge v-if="enabledHosts[s.name]" variant="secondary" class="ml-1">
                          exposed
                        </Badge>
                      </td>
                      <td class="py-2 pr-2">
                        <Input
                          v-model="hostInputs[s.name]"
                          :placeholder="suggestedHost(s.name)"
                          :disabled="!!enabledHosts[s.name]"
                        />
                      </td>
                      <td class="py-2 pl-2 text-right">
                        <Button
                          v-if="enabledHosts[s.name]"
                          variant="ghost"
                          size="sm"
                          :disabled="busy !== null || connected !== true"
                          @click="doDisableSite(s.name)"
                        >
                          <Spinner v-if="busy === `enable:${s.name}`" class="mr-1.5 size-3.5" />
                          Remove
                        </Button>
                        <Button
                          v-else
                          variant="outline"
                          size="sm"
                          :disabled="busy !== null || !(hostInputs[s.name] ?? '').trim() || connected !== true || !loggedIn"
                          @click="doEnableSite(s.name)"
                        >
                          <Spinner v-if="busy === `enable:${s.name}`" class="mr-1.5 size-3.5" />
                          Expose
                        </Button>
                      </td>
                    </tr>
                  </tbody>
                </table>
              </template>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>

    <Modal v-model:open="loginOpen" title="Connect Cloudflare account" size="lg">
      <p class="mb-3 text-sm text-muted-foreground">
        A browser window should have opened to authorize Yerd. If it didn't, use
        the button below.
      </p>
      <div
        v-if="loginUrl"
        class="mb-3 flex items-center justify-between gap-3 rounded-md border bg-muted/40 px-3 py-2"
      >
        <span class="truncate font-mono text-xs text-muted-foreground">{{ loginUrl }}</span>
        <Button size="sm" variant="outline" class="shrink-0" @click="openExternal(loginUrl)">
          <Globe class="mr-1.5 size-3.5" /> Open in browser
        </Button>
      </div>
      <pre
        ref="loginBox"
        class="h-64 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ loginLog.join("\n") || "Waiting for Cloudflare login…" }}</pre>
      <template #footer="{ close }">
        <Spinner v-if="busy === 'login'" class="size-4" />
        <Button :disabled="busy === 'login'" @click="close">Done</Button>
      </template>
    </Modal>

    <Modal v-model:open="logOpen" title="Installing cloudflared" size="lg">
      <pre
        ref="logBox"
        class="h-72 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ installLog.join("\n") || "Starting…" }}</pre>
      <template #footer="{ close }">
        <Spinner v-if="busy === 'install'" class="size-4" />
        <Button :disabled="busy === 'install'" @click="close">Done</Button>
      </template>
    </Modal>

    <Modal
      v-model:open="deleteTunnelOpen"
      title="Delete named tunnel"
      @update:open="(v: boolean) => { if (!v) deleteTunnelTarget = null; }"
    >
      <p class="text-sm text-muted-foreground">
        Delete <strong class="font-mono text-foreground">{{ deleteTunnelTarget }}</strong>?
        This removes it from your Cloudflare account and takes any exposed sites
        offline. Any DNS records you created for it may need removing in the
        Cloudflare dashboard.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button variant="destructive" @click="confirmDeleteTunnel(close)">Delete</Button>
      </template>
    </Modal>
  </div>
</template>
