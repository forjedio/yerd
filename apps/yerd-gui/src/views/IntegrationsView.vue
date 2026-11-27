<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import { useRouter } from "vue-router";
import { CloudDownload, Plus, Share2, SlidersHorizontal, Trash2, Wrench } from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import CloudflareLoginDialog from "@/components/share/CloudflareLoginDialog.vue";
import ShareInspector from "@/components/share/ShareInspector.vue";
import ShareList from "@/components/share/ShareList.vue";
import ShareProviderCards from "@/components/share/ShareProviderCards.vue";
import ShareSiteDialog, { type ShareRequest } from "@/components/share/ShareSiteDialog.vue";
import TunnelProvidersDialog from "@/components/share/providers/TunnelProvidersDialog.vue";
import AsyncState from "@/components/ui/AsyncState.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import { useDaemon } from "@/composables/useDaemon";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { useWideLayout } from "@/composables/useWideLayout";
import {
  cloudflaredLogin,
  createNamedTunnel,
  deleteNamedTunnel,
  IpcError,
  jobCancel,
  listNamedTunnels,
  listSites,
  listTunnelProviders,
  openInBrowser,
  pollJobToEnd,
  routeTunnelDns,
  setSiteTunnel,
  setTunnelPassword,
  startNamedTunnel,
  startQuickTunnel,
  stopTunnel,
  tunnelStatus,
} from "@/ipc/client";
import type {
  CloudflaredStatus,
  NamedTunnelMeta,
  Site,
  TunnelInfo,
  TunnelProvider,
  TunnelProviderStatus,
} from "@/ipc/types";
import {
  canStartNamed,
  checkProviderStart,
  CLEANUP_UNKNOWN_MESSAGE,
  cleanupFailedMessage,
  filterShares,
  loginUrlFrom,
  namedEntry,
  removeTunnelCopy,
  type ShareFilter,
  type ShareItem,
  shareItems,
  shareKey,
  siblingShares,
  unsharedProviders,
  unsupportedProviderMessage,
} from "@/lib/share";
import { copyUrlWithToast } from "@/lib/toastActions";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";

const toast = useToast();
const { connected, report, refresh: refreshDaemon } = useDaemon();
const router = useRouter();
const wide = useWideLayout();

/** A long-running action in flight, e.g. `share:ngrok:app`, `stop:ngrok:app`,
 *  `enable:app` or `create`. */
const busy = ref<string | null>(null);

const tld = computed(() => report.value?.tld ?? "test");

type SharePageData = {
  cloudflared: CloudflaredStatus;
  tunnels: TunnelInfo[];
  sites: Site[];
  named: Awaited<ReturnType<typeof listNamedTunnels>>;
};

/** The named-tunnel list is fetched even when logged out: the daemon returns the
 *  saved tunnel and site mappings without an account (only the zone needs one),
 *  so a logged-out user keeps control of an existing tunnel. */
async function fetchSharePage(): Promise<SharePageData> {
  const [status, siteList, named] = await Promise.all([tunnelStatus(), listSites(), listNamedTunnels()]);
  return { cloudflared: status.cloudflared, tunnels: status.tunnels, sites: siteList, named };
}

const { data, loading, refresh, mutate, error: resourceError } = useResource("share-page", fetchSharePage);

const cloudflared = computed<CloudflaredStatus | null>(() => data.value?.cloudflared ?? null);
const tunnels = computed<TunnelInfo[]>(() => data.value?.tunnels ?? []);
const sites = computed<Site[]>(() => data.value?.sites ?? []);
const namedTunnels = computed<NamedTunnelMeta[]>(() => data.value?.named.tunnels ?? []);
/** The sites mapped onto the named tunnel, with their hostnames. */
const namedSites = computed(() => data.value?.named.sites ?? []);
/** The authorized Cloudflare zone, once resolved from the account cert. */
const authorizedDomain = computed(() => data.value?.named.zone ?? null);

/** Provider readiness for the cards, the dialog and the inspector. Not part of
 *  the 5 s poll, since a PATH copy's version probe runs on every fetch; it is
 *  refreshed after the providers sheet closes and after each share or stop. */
const {
  data: providerData,
  refresh: refreshProviderData,
  error: providerFetchError,
} = useResource("tunnel-providers", listTunnelProviders);
const providers = computed<TunnelProviderStatus[]>(() => providerData.value?.providers ?? []);
const defaultProvider = computed<TunnelProvider>(() => providerData.value?.default_provider ?? "cloudflare");

function refreshProviders(): void {
  void refreshProviderData({ force: true });
}

/** Why the provider list couldn't load, while there is no list to show. A
 *  failed revalidation over a loaded list keeps the list. */
const providersError = computed(() =>
  providerData.value ? null : (providerFetchError.value?.message ?? null),
);

const installed = computed(() => cloudflared.value?.installed ?? false);
/** Whether any provider's binary is available; cloudflared alone until the
 *  provider list has loaded. */
const anyInstalled = computed(() =>
  providers.value.length ? providers.value.some((p) => p.readiness !== "not_installed") : installed.value,
);
const loggedIn = computed(() => cloudflared.value?.logged_in ?? false);
const entry = computed(() => namedEntry(tunnels.value));
const exposedSites = computed(() => namedSites.value.map((s) => s.site));
const canStart = computed(() => canStartNamed(entry.value, exposedSites.value.length));
const tunnel = computed(() => namedTunnels.value[0] ?? null);
const domainReady = computed(() => loggedIn.value && tunnel.value !== null);

/** Only a cold-load failure replaces the screen; a failed revalidation over
 *  cached data keeps it. */
const error = computed(() => (data.value ? null : (resourceError.value?.message ?? null)));

/** Fold an authoritative tunnel-status response into the cache. Writing through
 *  `mutate` bumps useResource's epoch, so an older poll can't revert it. */
function applyTunnelStatus(r: { tunnels: TunnelInfo[]; cloudflared: CloudflaredStatus }): void {
  mutate((cur) => (cur ? { ...cur, tunnels: r.tunnels, cloudflared: r.cloudflared } : cur));
}

/** Optimistically add or clear a site's hostname mapping in the cache. */
function setNamedSite(site: string, hostname: string | null): void {
  mutate((cur) => {
    if (!cur?.named) return cur;
    const prev = cur.named.sites.find((s) => s.site === site);
    const rest = cur.named.sites.filter((s) => s.site !== site);
    if (hostname) rest.push({ site, hostname, protected: prev?.protected });
    return { ...cur, named: { ...cur.named, sites: rest } };
  });
}

/** Force a revalidation, for post-write refreshes. */
async function reload(): Promise<void> {
  await refresh({ force: true });
}

const items = computed(() => shareItems(tunnels.value, namedSites.value));
const filter = ref<ShareFilter>("all");

// ── selection ──
const selectedKey = ref<string | null>(null);
const drawerOpen = ref(false);
/** The shares the list shows; only these can be selected. */
const visible = computed(() => filterShares(items.value, filter.value));
const selected = computed(() => visible.value.find((i) => i.key === selectedKey.value) ?? null);
const selectedSite = computed(() => sites.value.find((s) => s.name === selected.value?.site));
const selectedProvider = computed(
  () => providers.value.find((p) => p.provider === selected.value?.provider) ?? null,
);
const siblings = computed(() => (selected.value ? siblingShares(items.value, selected.value) : []));
const unshared = computed(() =>
  selected.value ? unsharedProviders(items.value, selected.value.site, providers.value) : [],
);

/** Keep the selection on a share the list shows: the wide layout falls back
 *  to the first visible share, the compact one clears it. */
watch(
  [visible, wide],
  ([list, isWide]) => {
    if (selectedKey.value && list.some((i) => i.key === selectedKey.value)) return;
    selectedKey.value = isWide ? (list[0]?.key ?? null) : null;
  },
  { immediate: true },
);

watch(selected, (s) => {
  if (!s) drawerOpen.value = false;
});

watch(wide, (isWide) => {
  if (isWide) drawerOpen.value = false;
});

/** A row click: selects it, and in the compact table opens the drawer. */
function select(key: string): void {
  selectedKey.value = key;
  if (!wide.value) drawerOpen.value = true;
}

// ── cloudflared ──
/** cloudflared is installed, updated and removed in Tooling; Share only links there. */
function goToTooling(): void {
  void router.push("/tooling");
}

// ── Cloudflare login ──
const loginOpen = ref(false);
const loginLines = ref<string[]>([]);
const loginUrl = computed(() => loginUrlFrom(loginLines.value));

/** Closing the dialog (or leaving the page) while the login still runs cancels
 *  the daemon job: it holds the tunnel lock until cloudflared exits, which
 *  would otherwise stall every other tunnel action for up to five minutes. */
async function doLogin(): Promise<void> {
  busy.value = "login";
  loginLines.value = [];
  loginOpen.value = true;
  try {
    const jobId = await cloudflaredLogin();
    const final = await pollJobToEnd(
      jobId,
      (lines) => {
        loginLines.value = [...loginLines.value, ...lines];
      },
      () => loginOpen.value,
    );
    if (final.state === "running") {
      await jobCancel(jobId).catch((e: unknown) =>
        toast.error("Couldn't cancel the Cloudflare login", (e as IpcError).message),
      );
    }
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

// ── named tunnel ──
async function doCreateTunnel(name: string): Promise<void> {
  busy.value = "create";
  try {
    await createNamedTunnel(name);
    await reload();
    toast.success(`Created tunnel ${name}`);
  } catch (e) {
    toast.error(`Couldn't create ${name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

const deleteOpen = ref(false);

/** Delete the named tunnel from the account. */
async function confirmDeleteTunnel(): Promise<void> {
  const name = tunnel.value?.name;
  if (!name) return;
  busy.value = "delete-tunnel";
  try {
    await deleteNamedTunnel(name);
    await reload();
    void refreshDaemon();
    toast.success(`Deleted tunnel ${name}`);
  } catch (e) {
    toast.error(`Couldn't delete ${name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

/** Start the named tunnel when sites are exposed but nothing is running (for
 *  example after `yerd tunnel unpublish`), or retry a failed one. Idempotent on
 *  the daemon side. */
async function doStartNamed(): Promise<void> {
  busy.value = "named";
  try {
    const r = await startNamedTunnel();
    applyTunnelStatus(r);
    const next = namedEntry(r.tunnels);
    if (next?.state === "running") {
      toast.success("Tunnel online");
    } else if (next?.state === "failed") {
      toast.info("Tunnel retrying", next.last_error ?? "It keeps retrying in the background");
    } else {
      toast.info("Tunnel connecting", "It shows as online once Cloudflare's edge is reached");
    }
  } catch (e) {
    toast.error("Couldn't start the tunnel", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

/** Expose a site: save its hostname, then route DNS to the named tunnel, rolling
 *  the mapping back if DNS fails. The daemon starts the tunnel once a hostname is set. */
async function doEnableSite(site: string, hostname: string): Promise<boolean> {
  const named = tunnel.value;
  if (!hostname || !named) return false;
  busy.value = `enable:${site}`;
  try {
    await setSiteTunnel(site, hostname);
    try {
      await routeTunnelDns(named.name, hostname);
    } catch (routeErr) {
      const rolledBack = await setSiteTunnel(site, null)
        .then(() => true)
        .catch(() => false);
      if (!rolledBack) await reload();
      throw routeErr;
    }
    setNamedSite(site, hostname);
    await reload();
    void refreshDaemon();
    toast.success(`Exposed ${site}`, `https://${hostname}`);
    return true;
  } catch (e) {
    toast.error(`Couldn't expose ${site}`, (e as IpcError).message);
    return false;
  } finally {
    busy.value = null;
  }
}

/** Remove a site from the domain: clear its hostname mapping. The daemon stops
 *  the named tunnel once the last hostname is cleared. */
async function doDisableSite(site: string): Promise<void> {
  busy.value = `enable:${site}`;
  try {
    await setSiteTunnel(site, null);
    setNamedSite(site, null);
    await reload();
    void refreshDaemon();
    toast.success(`Removed ${site} from the tunnel`);
  } catch (e) {
    toast.error(`Couldn't remove ${site}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── per-site shares ──
async function doShare(site: string, provider: TunnelProvider, randomUrl: boolean): Promise<boolean> {
  busy.value = `share:${provider}:${site}`;
  const before = tunnels.value;
  try {
    const r = await startQuickTunnel(site, provider, randomUrl);
    const check = checkProviderStart(provider, before, r.tunnels);
    const stray = check.stopCloudflare;
    const stopped = stray ? await stopTunnel(stray, "cloudflare").catch(() => null) : null;
    applyTunnelStatus(stopped ?? r);
    void refreshDaemon();
    refreshProviders();
    if (check.unsupported) {
      toast.error(`Couldn't share ${site}`, unsupportedProviderMessage(provider));
      if (stray && !stopped) toast.error(`Couldn't stop the Cloudflare share of ${stray}`, cleanupFailedMessage(stray));
      if (check.cleanupUnknown) toast.error("Check your Cloudflare shares", CLEANUP_UNKNOWN_MESSAGE);
      return false;
    }
    const started = r.tunnels.find(
      (t) => t.kind === "quick" && t.site === site && (t.provider ?? "cloudflare") === provider,
    );
    if (started?.url) {
      toast.success(`Sharing ${site}`, started.url);
    } else if (started?.state === "failed") {
      toast.info(`Sharing ${site}`, started.last_error ?? "Retrying");
    } else {
      toast.success(`Sharing ${site}`, "Connecting - the URL appears once it's online");
    }
    if (started?.warnings?.length) {
      toast.info(`Check ${site} while it's shared`, started.warnings.join("\n"), 10000);
    }
    return true;
  } catch (e) {
    toast.error(`Couldn't share ${site}`, (e as IpcError).message);
    return false;
  } finally {
    busy.value = null;
  }
}

async function doStop(item: ShareItem): Promise<void> {
  const { site, provider } = item;
  busy.value = `stop:${provider}:${site}`;
  try {
    const r = await stopTunnel(site, provider);
    applyTunnelStatus(r);
    void refreshDaemon();
    refreshProviders();
    toast.success(`Stopped sharing ${site}`);
  } catch (e) {
    toast.error(`Couldn't stop ${site}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── Share a site ──
const shareOpen = ref(false);
const shareInitialSite = ref<string | null>(null);
const shareInitialProvider = ref<TunnelProvider | null>(null);

/** Open Share a site, preselecting a site and provider when given. */
function openShare(site: string | null = null, provider: TunnelProvider | null = null): void {
  shareInitialSite.value = site;
  shareInitialProvider.value = provider;
  shareOpen.value = true;
}

/** "Also share through <provider>" from the inspector: close the drawer, then
 *  open the dialog on the selected site and that provider. */
function alsoShare(provider: TunnelProvider): void {
  const site = selected.value?.site ?? null;
  drawerOpen.value = false;
  void nextTick(() => openShare(site, provider));
}
const shareInFlight = computed(
  () => !!busy.value && (busy.value.startsWith("share:") || busy.value.startsWith("enable:")),
);

/** Run the dialog's share; on success close it and select the new share. */
async function onShare(request: ShareRequest): Promise<void> {
  const ok =
    request.provider === "cloudflare" && request.kind === "domain"
      ? await doEnableSite(request.site, request.hostname ?? "")
      : await doShare(request.site, request.provider, request.randomUrl);
  if (!ok) return;
  shareOpen.value = false;
  if (filter.value !== "all" && filter.value !== request.provider) filter.value = "all";
  await nextTick();
  select(shareKey(request.provider, request.kind, request.site));
}

// ── Providers ──
const providersOpen = ref(false);
const providersInitial = ref<TunnelProvider | null>(null);

function openProviders(initial: TunnelProvider | null = null): void {
  providersInitial.value = initial;
  providersOpen.value = true;
}

/** Close any open dialog or drawer, then open the Providers sheet on `initial`,
 *  so two dialogs never stack (the sibling of `fromProviders`). */
function toProviders(initial: TunnelProvider | null = null): void {
  shareOpen.value = false;
  drawerOpen.value = false;
  void nextTick(() => openProviders(initial));
}

/** Close the Providers sheet, then open the next dialog, so two dialogs never stack. */
function fromProviders(next: () => void): void {
  providersOpen.value = false;
  void nextTick(next);
}

function onProvidersTooling(): void {
  fromProviders(goToTooling);
}

function onProvidersLogin(): void {
  fromProviders(() => void doLogin());
}

function onProvidersRemove(): void {
  fromProviders(() => {
    deleteOpen.value = true;
  });
}

/** Reload once the sheet closes, so the page reflects provider changes. */
watch(providersOpen, (open) => {
  if (open) return;
  void reload();
  refreshProviders();
});

// ── share passwords ──
const passwordOpen = ref(false);
const passwordSite = ref<string | null>(null);
const passwordInput = ref("");
const passwordProtected = ref(false);

function askPassword(site: string, isProtected: boolean): void {
  passwordSite.value = site;
  passwordInput.value = "";
  passwordProtected.value = isProtected;
  passwordOpen.value = true;
}

/** Forget the target site and any typed password whenever the modal closes. */
function onPasswordOpenChange(open: boolean): void {
  if (open) return;
  passwordSite.value = null;
  passwordInput.value = "";
}

/** Set (a non-empty value) or remove (`null`) the target site's share password. */
async function savePassword(password: string | null, close: () => void): Promise<void> {
  const site = passwordSite.value;
  if (!site) return;
  close();
  busy.value = `password:${site}`;
  try {
    await setTunnelPassword(site, password);
    await reload();
    toast.success(password === null ? `Removed the password for ${site}` : `Protected ${site}`);
  } catch (e) {
    toast.error(`Couldn't update the password for ${site}`, (e as IpcError).message);
  } finally {
    busy.value = null;
    passwordSite.value = null;
    passwordInput.value = "";
  }
}

async function copyUrl(url: string): Promise<void> {
  await copyUrlWithToast(url);
}

/** Open an external URL through the Tauri opener, surfacing any failure. */
function openExternal(url: string): void {
  void openInBrowser(url).catch(() => toast.error("Couldn't open link"));
}

/** Silent revalidation while a tunnel exists, so health changes show without a
 *  manual reload. Skipped while an action is in flight so optimistic state holds.
 *  Not forced: the first tick lands on mount and joins useResource's own fetch. */
async function refreshTunnels(): Promise<void> {
  if (busy.value !== null || tunnels.value.length === 0) return;
  await refresh();
}

usePoll(refreshTunnels, 5000);

onUnmounted(() => {
  loginOpen.value = false;
});
onUnmounted(registerViewActions({ refresh: () => void reload() }));

const shareDisabled = computed(() => connected.value !== true || busy.value !== null);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Share" docs="/guide/sharing">
      <template #actions>
        <Button
          v-if="wide"
          variant="outline"
          size="sm"
          :disabled="!data"
          data-testid="open-providers"
          @click="openProviders()"
        >
          <SlidersHorizontal /> Providers
        </Button>
        <Button
          v-else
          variant="outline"
          size="icon"
          class="size-8"
          aria-label="Providers"
          title="Providers"
          :disabled="!data"
          data-testid="open-providers"
          @click="openProviders()"
        >
          <SlidersHorizontal />
        </Button>
        <Button size="sm" :disabled="shareDisabled" data-testid="open-share" @click="openShare()">
          <Plus /> Share a site
        </Button>
      </template>
    </PageHeader>

    <div class="min-h-0 flex-1">
      <AsyncState
        :loading="loading && !data"
        :error="error"
        :empty="!anyInstalled || !items.length"
        pad="py-16"
        @retry="reload"
      >
        <template #empty>
          <div class="flex h-full flex-col items-center justify-center px-6 py-16 text-center" data-testid="share-empty">
            <template v-if="!anyInstalled">
              <span class="flex size-11 items-center justify-center rounded-xl border bg-card text-muted-foreground">
                <CloudDownload class="size-5" aria-hidden="true" />
              </span>
              <p class="mt-4 text-[15px] font-semibold">Sharing needs a tunnel provider</p>
              <p class="mt-1.5 max-w-sm text-[13px] text-muted-foreground">
                Install cloudflared, ngrok or zrok from Tooling.
              </p>
              <Button
                size="sm"
                class="mt-5"
                :disabled="connected !== true"
                data-testid="open-tooling"
                @click="goToTooling"
              >
                <Wrench /> Open Tooling
              </Button>
            </template>
            <template v-else>
              <span class="flex size-11 items-center justify-center rounded-xl border bg-card text-muted-foreground">
                <Share2 class="size-5" aria-hidden="true" />
              </span>
              <p class="mt-4 text-[15px] font-semibold">Nothing shared yet</p>
              <p class="mt-1.5 max-w-sm text-[13px] text-muted-foreground">
                Share a site to get a public URL you can send to anyone.
              </p>
              <Button size="sm" class="mt-5" :disabled="shareDisabled" @click="openShare()">
                <Plus /> Share a site
              </Button>
            </template>
          </div>
        </template>

        <div class="flex h-full min-h-0" :data-layout="wide ? 'wide' : 'compact'">
          <div :class="wide ? 'flex w-[360px] shrink-0 flex-col pl-4 pt-4' : 'flex min-w-0 flex-1 flex-col pl-4 pt-4'">
            <ShareProviderCards
              :providers="providers"
              :error="providersError"
              @manage="toProviders()"
              @retry="refreshProviders"
            />
            <ShareList
              v-model:filter="filter"
              class="min-h-0 flex-1"
              :layout="wide ? 'list' : 'table'"
              :items="items"
              :selected="selectedKey"
              @select="select"
            />
          </div>

          <div :class="wide ? 'min-w-0 flex-1 border-l' : 'hidden'" data-testid="inspector-column">
            <ShareInspector
              :mode="wide ? 'panel' : 'drawer'"
              :shown="wide ? !!selected : drawerOpen && !!selected"
              :item="selected"
              :site="selectedSite"
              :tld="tld"
              :tunnel-name="tunnel?.name ?? null"
              :cloudflared="cloudflared"
              :provider-status="selectedProvider"
              :siblings="siblings"
              :unshared="unshared"
              :can-restart="canStart"
              :logged-in="loggedIn"
              :busy="busy"
              :connected="connected === true"
              @close="drawerOpen = false"
              @stop="selected && doStop(selected)"
              @also-share="alsoShare"
              @open-sibling="openExternal"
              @remove-from-domain="selected && doDisableSite(selected.site)"
              @restart="doStartNamed"
              @password="selected && askPassword(selected.site, selected.protected)"
              @copy="copyUrl"
              @open="openExternal"
            />
            <p v-if="wide && !selected" class="flex h-full items-center justify-center text-[13px] text-faint">
              Select a share to see its details.
            </p>
          </div>
        </div>
      </AsyncState>
    </div>

    <ShareSiteDialog
      v-model:open="shareOpen"
      :sites="sites"
      :items="items"
      :providers="providers"
      :providers-error="providersError"
      :default-provider="defaultProvider"
      :initial-site="shareInitialSite"
      :initial-provider="shareInitialProvider"
      :zone="authorizedDomain"
      :domain-ready="domainReady"
      :tld="tld"
      :busy="shareInFlight"
      :locked="busy !== null || connected !== true"
      @share="onShare"
      @setup="toProviders"
      @retry-providers="refreshProviders"
    />

    <TunnelProvidersDialog
      v-model:open="providersOpen"
      :cloudflared="cloudflared"
      :zone="authorizedDomain"
      :tunnel="tunnel"
      :entry="entry"
      :exposed-count="exposedSites.length"
      :busy="busy"
      :connected="connected === true"
      :initial="providersInitial"
      @tooling="onProvidersTooling"
      @login="onProvidersLogin"
      @create="doCreateTunnel"
      @start="doStartNamed"
      @remove="onProvidersRemove"
      @open-url="openExternal"
    />

    <CloudflareLoginDialog
      v-model:open="loginOpen"
      :lines="loginLines"
      :url="loginUrl"
      :running="busy === 'login'"
      @open-url="openExternal"
    />

    <ConfirmDialog
      v-model:open="deleteOpen"
      :title="`Remove tunnel ${tunnel?.name ?? ''}?`"
      confirm-label="Remove tunnel"
      tone="danger"
      :icon="Trash2"
      @confirm="confirmDeleteTunnel"
    >
      {{ removeTunnelCopy(exposedSites) }}
    </ConfirmDialog>

    <Modal v-model:open="passwordOpen" title="Share password" @update:open="onPasswordOpenChange">
      <p class="mb-3 text-sm text-muted-foreground">
        Visitors to <strong class="font-mono text-foreground">{{ passwordSite }}</strong>
        over the tunnel must sign in with this password (any username). Local
        <span class="font-mono">.test</span> access is unaffected.
      </p>
      <Input
        id="share-password"
        v-model="passwordInput"
        type="password"
        autocomplete="new-password"
        placeholder="Share password"
      />
      <template #footer="{ close }">
        <Button
          v-if="passwordProtected"
          variant="ghost"
          class="mr-auto text-destructive hover:bg-destructive/10 hover:text-destructive"
          @click="savePassword(null, close)"
        >
          Remove password
        </Button>
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button :disabled="passwordInput.length === 0" @click="savePassword(passwordInput, close)">Save</Button>
      </template>
    </Modal>
  </div>
</template>
