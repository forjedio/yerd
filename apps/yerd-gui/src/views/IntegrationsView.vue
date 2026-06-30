<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import { Cloud, Copy, Download, Globe, Square } from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  cloudflaredLogin,
  createNamedTunnel,
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

const loading = ref(true);
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
// The per-site "Shared sites" table is Quick-only; the named tunnel is managed
// in its own card.
const quickTunnels = computed(() => tunnels.value.filter((t) => t.kind === "quick"));

// Sites not already tunnelled, for the share picker.
const shareableSites = computed(() => {
  const live = new Set(tunnels.value.map((t) => t.site));
  return sites.value.filter((s) => !live.has(s.name));
});

const shareOptions = computed(() =>
  shareableSites.value.map((s) => ({ value: s.name, label: s.name })),
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

async function load(): Promise<void> {
  loading.value = true;
  try {
    const [status, siteList] = await Promise.all([tunnelStatus(), listSites()]);
    cloudflared.value = status.cloudflared;
    tunnels.value = status.tunnels;
    sites.value = siteList;
    if (!shareableSites.value.some((s) => s.name === shareSite.value)) {
      shareSite.value = shareableSites.value[0]?.name ?? "";
    }
    if (loggedIn.value) {
      const named = await listNamedTunnels();
      namedTunnels.value = named.tunnels;
      enabledHosts.value = Object.fromEntries(named.sites.map((s) => [s.site, s.hostname]));
      // Seed each site's input from its enabled hostname (keep edits otherwise).
      for (const s of siteList) {
        if (hostInputs.value[s.name] === undefined) {
          hostInputs.value[s.name] = enabledHosts.value[s.name] ?? "";
        }
      }
    } else {
      namedTunnels.value = [];
      enabledHosts.value = {};
    }
  } catch (e) {
    toast.error("Couldn't load tunnels", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

// ── named tunnels (Phase 2) ───────────────────────────────────────────────

const loginOpen = ref(false);
const loginLog = ref<string[]>([]);
const loginBox = ref<HTMLElement | null>(null);

async function appendLogin(lines: string[]): Promise<void> {
  for (const line of lines) {
    // The daemon prefixes the auth URL; open it in the system browser.
    const m = line.match(/https:\/\/\S*cloudflare\.com\S*/);
    if (m) void openInBrowser(m[0]);
  }
  loginLog.value.push(...lines);
  await nextTick();
  const el = loginBox.value;
  if (el) el.scrollTop = el.scrollHeight;
}

async function doLogin(): Promise<void> {
  busy.value = "login";
  loginLog.value = [];
  loginOpen.value = true;
  try {
    const jobId = await cloudflaredLogin();
    const final = await pollJobToEnd(jobId, (lines) => void appendLogin(lines), () => loginOpen.value);
    await load();
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
    namedTunnels.value = (await listNamedTunnels()).tunnels;
    toast.success(`Created tunnel ${name}`);
  } catch (e) {
    toast.error(`Couldn't create ${name}`, (e as IpcError).message);
  } finally {
    busy.value = null;
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
      // Routing failed (e.g. the domain isn't on this Cloudflare account), so
      // roll back the persisted mapping rather than leave a site flagged
      // "exposed" with no DNS record behind it.
      await setSiteTunnel(site, null).catch(() => {});
      throw routeErr;
    }
    enabledHosts.value = { ...enabledHosts.value, [site]: hostname };
    toast.success(`Exposed ${site}`, `https://${hostname} - start the tunnel to apply`);
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
    const next = { ...enabledHosts.value };
    delete next[site];
    enabledHosts.value = next;
    toast.success(`Removed ${site} from the tunnel`);
  } catch (e) {
    toast.error(`Couldn't remove ${site}`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

/** (Re)start the consolidated named tunnel serving all enabled sites. */
async function doStartNamed(): Promise<void> {
  busy.value = "named";
  try {
    const r = await startNamedTunnel();
    tunnels.value = r.tunnels;
    cloudflared.value = r.cloudflared;
    toast.success("Named tunnel started");
  } catch (e) {
    toast.error("Couldn't start named tunnel", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function doStopNamed(): Promise<void> {
  busy.value = "named";
  try {
    const r = await stopNamedTunnel();
    tunnels.value = r.tunnels;
    cloudflared.value = r.cloudflared;
    toast.success("Named tunnel stopped");
  } catch (e) {
    toast.error("Couldn't stop named tunnel", (e as IpcError).message);
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
    await load();
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
    tunnels.value = r.tunnels;
    cloudflared.value = r.cloudflared;
    // The shared site drops out of shareableSites; re-point the picker so the
    // model and the rendered <select> don't diverge (matches load()).
    if (!shareableSites.value.some((s) => s.name === shareSite.value)) {
      shareSite.value = shareableSites.value[0]?.name ?? "";
    }
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
    tunnels.value = r.tunnels;
    cloudflared.value = r.cloudflared;
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

// Light refresh of just the tunnel list while any tunnel is live, so a
// cloudflared child that dies (state -> "failed") becomes visible without a
// manual reload. Skipped while a long-running op is in flight to avoid clobbering
// optimistic UI; cleared on unmount.
let poll: ReturnType<typeof setInterval> | null = null;
async function refreshTunnels(): Promise<void> {
  if (busy.value !== null || tunnels.value.length === 0) return;
  try {
    const status = await tunnelStatus();
    cloudflared.value = status.cloudflared;
    tunnels.value = status.tunnels;
  } catch {
    // Transient; the next tick retries.
  }
}

onMounted(() => {
  void load();
  poll = setInterval(() => void refreshTunnels(), 5000);
});
onUnmounted(() => {
  if (poll !== null) clearInterval(poll);
  logOpen.value = false;
  loginOpen.value = false;
});
onUnmounted(registerViewActions({ refresh: () => void load() }));
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Integrations"
      subtitle="Publish local sites to the internet via Cloudflare Tunnel"
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
              <Badge v-if="installed" variant="secondary">
                cloudflared {{ cloudflared?.version ?? "installed" }}
              </Badge>
              <span v-else class="text-muted-foreground">
                cloudflared is not installed yet.
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
          <div class="mb-4 flex items-end gap-2">
            <div class="flex-1">
              <label class="mb-1 block text-xs text-muted-foreground">Site</label>
              <Select
                v-model="shareSite"
                :options="shareOptions"
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

          <p v-if="quickTunnels.length === 0" class="py-4 text-sm text-muted-foreground">
            No active tunnels.
          </p>

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
                    @click="openInBrowser(t.url ?? `https://${t.hostname}`)"
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
            Publish a site at a stable hostname on your own Cloudflare domain.
            Requires a Cloudflare account and a domain managed by Cloudflare.
          </CardDescription>
        </CardHeader>

        <CardContent>
          <div v-if="!loggedIn" class="flex items-center justify-between gap-4">
            <span class="text-sm text-muted-foreground">
              Not connected to a Cloudflare account.
            </span>
            <Button :disabled="busy !== null || connected !== true" @click="doLogin">
              <Cloud class="mr-1.5 size-3.5" /> Connect Cloudflare account
            </Button>
          </div>

          <div v-else class="space-y-5">
            <div class="flex items-center gap-2">
              <Badge variant="secondary">Cloudflare connected</Badge>
            </div>

            <div>
              <label class="mb-1 block text-xs text-muted-foreground">Create a tunnel</label>
              <div class="flex items-end gap-2">
                <Input
                  v-model="newTunnelName"
                  placeholder="tunnel name"
                  class="flex-1"
                  :disabled="namedTunnels.length > 0"
                />
                <Button
                  :disabled="busy !== null || !newTunnelName.trim() || namedTunnels.length > 0 || connected !== true"
                  @click="doCreateTunnel"
                >
                  <Spinner v-if="busy === 'create'" class="mr-1.5 size-3.5" />
                  Create
                </Button>
              </div>
              <ul v-if="namedTunnels.length" class="mt-2 space-y-1 text-xs text-muted-foreground">
                <li v-for="t in namedTunnels" :key="t.uuid" class="font-mono">
                  {{ t.name }} · {{ t.uuid }}
                </li>
              </ul>
              <p v-if="namedTunnels.length" class="mt-1 text-xs text-muted-foreground">
                Yerd uses one tunnel to serve every exposed site.
              </p>
            </div>

            <div>
              <div class="mb-2 flex items-center justify-between">
                <label class="text-xs text-muted-foreground">
                  Choose which sites to expose
                </label>
                <div class="flex items-center gap-2">
                  <Badge v-if="namedRunning" variant="secondary">running</Badge>
                  <Button
                    v-if="namedRunning"
                    variant="outline"
                    size="sm"
                    :disabled="busy !== null"
                    @click="doStopNamed"
                  >
                    <Spinner v-if="busy === 'named'" class="mr-1.5 size-3.5" />
                    <Square v-else class="mr-1.5 size-3.5" />
                    Stop
                  </Button>
                  <Button
                    size="sm"
                    :disabled="busy !== null || namedTunnels.length === 0"
                    @click="doStartNamed"
                  >
                    <Spinner v-if="busy === 'named'" class="mr-1.5 size-3.5" />
                    <Globe v-else class="mr-1.5 size-3.5" />
                    {{ namedRunning ? "Restart" : "Start tunnel" }}
                  </Button>
                </div>
              </div>

              <p v-if="namedTunnels.length === 0" class="text-xs text-muted-foreground">
                Create a tunnel above first.
              </p>
              <p v-else-if="sites.length === 0" class="text-xs text-muted-foreground">
                No sites yet.
              </p>

              <table v-else class="w-full text-sm">
                <tbody>
                  <tr v-for="s in sites" :key="s.name" class="border-b last:border-0">
                    <td class="py-2 pr-2 font-medium text-foreground">
                      {{ s.name }}
                      <Badge v-if="enabledHosts[s.name]" variant="secondary" class="ml-1">
                        exposed
                      </Badge>
                    </td>
                    <td class="py-2 pr-2">
                      <Input
                        v-model="hostInputs[s.name]"
                        placeholder="app.example.com"
                        :disabled="!!enabledHosts[s.name]"
                      />
                    </td>
                    <td class="py-2 pl-2 text-right">
                      <Button
                        v-if="enabledHosts[s.name]"
                        variant="ghost"
                        size="sm"
                        :disabled="busy !== null"
                        @click="doDisableSite(s.name)"
                      >
                        <Spinner v-if="busy === `enable:${s.name}`" class="mr-1.5 size-3.5" />
                        Remove
                      </Button>
                      <Button
                        v-else
                        variant="outline"
                        size="sm"
                        :disabled="busy !== null || !(hostInputs[s.name] ?? '').trim()"
                        @click="doEnableSite(s.name)"
                      >
                        <Spinner v-if="busy === `enable:${s.name}`" class="mr-1.5 size-3.5" />
                        Expose
                      </Button>
                    </td>
                  </tr>
                </tbody>
              </table>
              <p class="mt-2 text-xs text-muted-foreground">
                Exposing a site routes its hostname; Start (or Restart) applies the
                current set to the live tunnel.
              </p>
            </div>
          </div>
        </CardContent>
      </Card>
    </div>

    <Modal v-model:open="loginOpen" title="Connect Cloudflare account" size="lg">
      <pre
        ref="loginBox"
        class="h-72 overflow-y-auto whitespace-pre-wrap rounded-lg bg-zinc-950 p-3 font-mono text-[11px] leading-relaxed text-zinc-200"
      >{{ loginLog.join("\n") || "Opening Cloudflare login in your browser…" }}</pre>
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
  </div>
</template>
