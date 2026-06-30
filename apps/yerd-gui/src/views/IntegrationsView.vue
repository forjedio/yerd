<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from "vue";
import { Copy, Download, Globe, Square } from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  installCloudflaredStreamed,
  IpcError,
  listSites,
  pollJobToEnd,
  startQuickTunnel,
  stopTunnel,
  tunnelStatus,
} from "@/ipc/client";
import type { CloudflaredStatus, Site, TunnelInfo } from "@/ipc/types";

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
  } catch (e) {
    toast.error("Couldn't load tunnels", (e as IpcError).message);
  } finally {
    loading.value = false;
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

onMounted(load);
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

          <p v-if="tunnels.length === 0" class="py-4 text-sm text-muted-foreground">
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
              <tr v-for="t in tunnels" :key="t.site" class="border-b last:border-0">
                <td class="py-3 pr-4 font-medium text-foreground">
                  {{ t.site }}
                  <Badge v-if="t.state === 'failed'" variant="outline" class="ml-1">
                    failed
                  </Badge>
                </td>
                <td class="py-3 pr-4">
                  <a
                    v-if="t.url ?? t.hostname"
                    :href="t.url ?? `https://${t.hostname}`"
                    target="_blank"
                    rel="noopener"
                    class="font-mono text-xs text-brand hover:underline"
                  >
                    {{ t.url ?? t.hostname }}
                  </a>
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
    </div>

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
