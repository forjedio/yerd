<script setup lang="ts">
import {
  ArrowRight,
  ArrowUpRight,
  Database,
  Info,
  LayoutGrid,
  Lock,
  LockOpen,
  Mail,
  Rocket,
  RotateCw,
  ShieldAlert,
  ShieldCheck,
  Square,
  SquareCode,
} from "lucide-vue-next";
import type { Component } from "vue";
import { computed, onMounted, onUnmounted, ref, watch } from "vue";
import { RouterLink } from "vue-router";

import DaemonDownHero from "@/components/DaemonDownHero.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusPill, { type Tone } from "@/components/StatusPill.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import { useDaemon } from "@/composables/useDaemon";
import { useOnboarding } from "@/composables/useOnboarding";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import {
  daemonVersionConflict,
  IpcError,
  openInBrowser,
  restartDaemon,
  sitesAndParked,
  stopDaemon,
} from "@/ipc/client";
import { useToast } from "@/composables/useToast";
import type { StatusReport } from "@/ipc/types";
import { openTitle, siteUrl } from "@/lib/siteUrl";
import { humaniseUptime } from "@/lib/utils";

// The home/dashboard. It reads the shared daemon report (no poller of its own)
// and shows the shared daemon-down hero, so it stays useful when the socket is
// gone - the same surface that celebrates "serving N sites" becomes the start button.
const { report, connected, refresh } = useDaemon();
const { relaunch } = useOnboarding();
const toast = useToast();

const r = computed(() => report.value);
const running = computed(() => connected.value === true);
const daemonPid = computed(() => report.value?.daemon_pid ?? null);

// ── daemon lifecycle (Stop / Restart; Start lives in the daemon-down hero) ──
const busy = ref<string | null>(null);

const stopDaemonOpen = ref(false);
async function confirmStopDaemon(close: () => void): Promise<void> {
  busy.value = "daemon";
  close();
  try {
    await stopDaemon();
    toast.success("Stopping daemon…");
  } catch (e) {
    toast.error("Couldn't stop the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refresh();
  }
}

const restartDaemonOpen = ref(false);
/**
 * Restart is fire-and-forget: the daemon acknowledges before it re-execs, so a
 * resolved call means the restart was accepted (the status poll handles the
 * connection drop), while a rejection is a genuine failure worth surfacing.
 */
async function confirmRestartDaemon(close: () => void): Promise<void> {
  busy.value = "restart:daemon";
  close();
  try {
    await restartDaemon();
    toast.info("Restarting daemon…", "It returns in a few seconds.");
  } catch (e) {
    toast.error("Couldn't restart the daemon", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}
// First paint, before the poll has resolved either way.
const connecting = computed(() => connected.value === null && !report.value);
const tld = computed(() => r.value?.tld ?? "test");

// ── live site list (for the console chips) ──
// Shared "sites" cache (same key + fetcher as the Sites view and the command
// palette). `immediate:false` so it never fetches while the daemon is down (the
// hero shows then); the `running`-gated triggers below drive it instead. A chip
// strip is non-critical, so a failed fetch just leaves the last-good list.
const { data: sitesData, refresh: reloadSites } = useResource("sites", sitesAndParked, {
  immediate: false,
});
const sites = computed(() => sitesData.value?.sites ?? []);
const sitesLoaded = computed(() => sitesData.value !== null);

// macOS: set when this (older) GUI refused to reconfigure a NEWER registered
// daemon. A GUI-vs-registered-daemon condition (independent of reachability), so
// it shows above every branch below.
const versionConflict = ref<string | null>(null);

onMounted(() => {
  if (running.value) void reloadSites();
  daemonVersionConflict()
    .then((v) => (versionConflict.value = v))
    .catch(() => {});
});
// Refetch on (re)connect. On daemon-down do nothing: the display is derived from
// the shared cache and the hero renders regardless, so clearing it would only
// hurt the Sites view / palette that share the entry.
watch(running, (up) => {
  if (up) void reloadSites();
});
usePoll(() => (running.value ? reloadSites() : Promise.resolve()), 5000);

onUnmounted(
  registerViewActions({
    refresh: () => {
      if (running.value) void reloadSites();
    },
  }),
);

// Once the real list is in, drive every count from it so the headline and the
// chips never disagree; fall back to the report's counts until then.
const siteCount = computed(() =>
  sitesLoaded.value
    ? sites.value.length
    : r.value
      ? r.value.sites.parked + r.value.sites.linked
      : 0,
);
const securedCount = computed(() =>
  sitesLoaded.value
    ? sites.value.filter((s) => s.secure).length
    : (r.value?.sites.secured ?? 0),
);

const SITE_CHIP_LIMIT = 14;
const sitePreview = computed(() => {
  // Secured first, then alphabetical - a stable, scannable order.
  return [...sites.value]
    .sort((a, b) => Number(b.secure) - Number(a.secure) || a.name.localeCompare(b.name))
    .slice(0, SITE_CHIP_LIMIT);
});
const moreCount = computed(() =>
  Math.max(0, sites.value.length - sitePreview.value.length),
);


// ── stat tiles ──
interface Tile {
  to: string;
  icon: Component;
  label: string;
  value: string;
  unit: string;
  sub: string;
}
const tiles = computed<Tile[]>(() => {
  const x = r.value;
  if (!x) return [];
  const out: Tile[] = [
    {
      to: "/php",
      icon: SquareCode,
      label: "PHP",
      value: String(x.php.length),
      unit: x.php.length === 1 ? "version" : "versions",
      sub: `default ${x.default_php}`,
    },
    {
      to: "/sites",
      icon: LayoutGrid,
      label: "Sites",
      value: String(siteCount.value),
      unit: siteCount.value === 1 ? "site" : "sites",
      sub: `${x.sites.parked} parked · ${x.sites.linked} linked`,
    },
  ];
  if (x.services && x.services.length) {
    const up = x.services.filter((s) => s.state === "running").length;
    out.push({
      to: "/services",
      icon: Database,
      label: "Services",
      value: String(up),
      unit: `/ ${x.services.length} up`,
      sub: x.services.map((s) => (s.site ? `${s.display_name} (${s.site})` : s.display_name)).join(" · "),
    });
  }
  if (x.mail) {
    out.push({
      to: "/mail",
      icon: Mail,
      label: "Mail",
      value: x.mail.enabled ? String(x.mail.count) : "Off",
      unit: x.mail.enabled ? "captured" : "",
      sub: x.mail.enabled ? `SMTP on port ${x.mail.port}` : "capture disabled",
    });
  }
  return out;
});

// ── system health (the three OS privileges, summarised) ──
const PRIVILEGED_PORT_CEILING = 1024;
function portsReady(x: StatusReport): boolean {
  const fellPrivileged =
    (x.http.requested < PRIVILEGED_PORT_CEILING && x.http.fell_back) ||
    (x.https.requested < PRIVILEGED_PORT_CEILING && x.https.fell_back);
  return !fellPrivileged || x.port_redirect === true;
}
function healthTone(v: boolean | null): Tone {
  if (v === true) return "ok";
  if (v === false) return "bad";
  return "unknown";
}
interface Health {
  key: string;
  label: string;
  value: boolean | null;
  yes: string;
  no: string;
}
const health = computed<Health[]>(() => {
  const x = r.value;
  if (!x) return [];
  return [
    { key: "trust", label: "Local CA", value: x.ca.trusted_system, yes: "Trusted", no: "Not trusted" },
    { key: "resolver", label: ".test resolver", value: x.resolver_installed, yes: "Installed", no: "Missing" },
    { key: "ports", label: "Ports 80/443", value: portsReady(x), yes: "Ready", no: "Fell back" },
  ];
});
const anyUnhealthy = computed(() => health.value.some((h) => h.value !== true));

const uptime = computed(() => (r.value ? humaniseUptime(r.value.uptime_secs) : ""));
const version = computed(() => r.value?.daemon_version ?? "");

// Nothing installed and nothing parked → the environment looks freshly set up
// (or wiped). Offer to re-run the guided onboarding. Driven off the live report,
// so it only evaluates while the daemon is up (the down branch shows the hero).
const emptyEnvironment = computed(
  () =>
    !!r.value &&
    r.value.php.length === 0 &&
    r.value.sites.parked === 0 &&
    r.value.sites.linked === 0,
);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Overview"
      subtitle="Your local environment at a glance"
      docs="/guide/desktop-app"
    />

    <div class="flex-1 space-y-4 overflow-y-auto p-6">
      <!-- Version conflict: this GUI is OLDER than the registered daemon. Shown
           above every branch - it's independent of daemon reachability. -->
      <div
        v-if="versionConflict"
        class="flex items-start gap-3 rounded-md border border-warning/40 bg-warning/10 p-4"
      >
        <ShieldAlert class="mt-0.5 size-5 shrink-0 text-warning" />
        <div class="min-w-0 flex-1">
          <p class="text-sm font-medium">This Yerd is older than your daemon</p>
          <p class="mt-1 text-sm text-muted-foreground">
            The background daemon is registered at version
            <span class="font-mono">{{ versionConflict }}</span
            >, newer than this app. Yerd won't reconfigure or downgrade it - update
            Yerd to {{ versionConflict }} or newer.
          </p>
        </div>
      </div>

      <!-- Connecting: first probe hasn't resolved yet. -->
      <div v-if="connecting" class="flex justify-center py-24">
        <Spinner class="size-6" />
      </div>

      <!-- Daemon down: the shared start affordance (also used on every blocked page). -->
      <DaemonDownHero v-else-if="!running" />

      <!-- Daemon up: the serving console. -->
      <template v-else>
        <!-- Degraded: the daemon couldn't bind its web ports, so nothing serves. -->
        <div
          v-if="r?.web_unbound"
          class="flex items-start gap-3 rounded-md border border-warning/40 bg-warning/10 p-4"
        >
          <ShieldAlert class="mt-0.5 size-5 shrink-0 text-warning" />
          <div class="min-w-0 flex-1">
            <p class="text-sm font-medium">Yerd isn't serving your sites</p>
            <p class="mt-1 text-sm text-muted-foreground">
              It couldn't bind its web ports ({{ r.web_unbound.http }}/{{
                r.web_unbound.https
              }}) - they're in use by another process. Change Yerd's ports to free
              ones to start serving.
            </p>
          </div>
          <RouterLink
            to="/general"
            class="shrink-0 self-center rounded-md bg-brand px-3 py-1.5 text-sm font-medium text-brand-foreground hover:bg-brand/90"
          >
            Change ports
          </RouterLink>
        </div>

        <!-- Empty environment (no PHP, nothing parked) → re-run guided setup. -->
        <div
          v-else-if="emptyEnvironment"
          class="flex items-start gap-3 rounded-md border border-brand/40 bg-brand/5 p-4"
        >
          <Rocket class="mt-0.5 size-5 shrink-0 text-brand" />
          <div class="min-w-0 flex-1">
            <p class="text-sm font-medium">Let's get you set up</p>
            <p class="mt-1 text-sm text-muted-foreground">
              You don't have any PHP versions installed or folders parked yet.
              Walk through the guided setup to install PHP and park your first
              projects folder.
            </p>
          </div>
          <Button class="shrink-0" @click="relaunch">
            <Rocket class="size-4" /> Load onboarding
          </Button>
        </div>

        <!-- Stat tiles → each links to its page. -->
        <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <RouterLink
            v-for="tile in tiles"
            :key="tile.to"
            :to="tile.to"
            class="group relative overflow-hidden rounded-lg border bg-card pl-5 pr-4 py-3.5 shadow-sm transition-colors hover:border-brand/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          >
            <span
              class="pointer-events-none absolute inset-y-0 left-0 w-[3px] bg-brand/50 transition-colors group-hover:bg-brand"
            />
            <div class="flex items-center gap-1.5">
              <component :is="tile.icon" class="size-3.5 text-muted-foreground" />
              <span class="text-[11px] font-medium uppercase tracking-wider text-muted-foreground">
                {{ tile.label }}
              </span>
              <ArrowUpRight
                class="ml-auto size-3.5 text-muted-foreground/40 transition-colors group-hover:text-brand"
              />
            </div>
            <p class="mt-2.5 flex items-baseline gap-1.5 leading-none">
              <span class="text-[2rem] font-semibold tabular-nums tracking-tight">
                {{ tile.value }}
              </span>
              <span class="text-xs text-muted-foreground">{{ tile.unit }}</span>
            </p>
            <p
              class="mt-3 truncate border-t pt-2 font-mono text-[11px] text-muted-foreground"
              :title="tile.sub"
            >
              {{ tile.sub }}
            </p>
          </RouterLink>
        </div>

        <Card class="relative overflow-hidden">
          <!-- A soft brand wash - the only place indigo gets a hero surface.
               A gradient fade rather than a blurred circle: `filter: blur()`
               forces this element onto its own GPU compositing layer, which on
               WebKitGTK (Linux) can make the window's software rounded-corner
               clip (`#app`'s `overflow: hidden`, since the window itself has
               no native corner mask) briefly fail to reapply on an unrelated
               repaint elsewhere in the tree, punching a hole to the desktop. -->
          <div
            class="pointer-events-none absolute -right-24 -top-24 size-72 rounded-full bg-[radial-gradient(circle,hsl(var(--brand)/0.08)_0%,transparent_70%)]"
            aria-hidden="true"
          />

          <div class="relative">
            <div class="flex items-center gap-2">
              <span class="relative flex size-2.5">
                <span
                  class="absolute inline-flex h-full w-full rounded-full bg-brand opacity-60 motion-safe:animate-ping"
                />
                <span class="relative inline-flex size-2.5 rounded-full bg-brand" />
              </span>
              <span
                class="text-xs font-medium uppercase tracking-wider text-muted-foreground"
              >
                Serving
              </span>
            </div>

            <h2 class="mt-2 text-2xl font-semibold tracking-tight">
              {{ siteCount }} {{ siteCount === 1 ? "site" : "sites" }}
              <span class="font-mono text-xl font-normal text-muted-foreground">
                .{{ tld }}
              </span>
            </h2>
            <p class="mt-1 text-sm text-muted-foreground">
              <template v-if="siteCount">
                {{ securedCount }} secured over HTTPS ·
              </template>
              default PHP
              <span class="font-mono text-foreground">{{ r?.default_php }}</span>
              <template v-if="version">
                · yerdd <span class="font-mono">{{ version }}</span>
              </template>
              <template v-if="uptime"> · up {{ uptime }}</template>
            </p>

            <!-- Console chips: every served domain, click to open. -->
            <div v-if="sitePreview.length" class="mt-5 flex flex-wrap gap-1.5">
              <button
                v-for="s in sitePreview"
                :key="s.name"
                class="group inline-flex items-center gap-1.5 rounded-md border bg-background px-2 py-1 text-xs transition-colors hover:border-brand/40 hover:bg-brand/5"
                :title="openTitle(s, r)"
                @click="openInBrowser(siteUrl(s, r))"
              >
                <Lock v-if="s.secure" class="size-3 text-success" />
                <LockOpen v-else class="size-3 text-muted-foreground" />
                <span class="font-mono">{{ s.name }}.{{ tld }}</span>
                <span class="font-mono text-muted-foreground">{{ s.php }}</span>
              </button>
              <RouterLink
                to="/sites"
                class="inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs font-medium text-brand transition-colors hover:text-brand/70"
              >
                {{ moreCount ? `+${moreCount} more` : "Manage" }}
                <ArrowRight class="size-3" />
              </RouterLink>
            </div>

            <!-- No sites: an invitation, not a blank. -->
            <div
              v-else-if="sitesLoaded"
              class="mt-5 rounded-md border border-dashed px-4 py-6 text-center text-sm text-muted-foreground"
            >
              No sites yet.
              <RouterLink to="/sites" class="font-medium text-brand hover:underline">
                Park a folder
              </RouterLink>
              to start serving.
            </div>
          </div>
        </Card>

        <!-- Daemon control - Stop/Restart. Start is owned by the daemon-down
             hero above (this branch only renders while the daemon is up). -->
        <Card class="p-4">
          <div class="flex flex-wrap items-center justify-between gap-x-8 gap-y-3">
            <div class="min-w-0 flex-1 basis-64">
              <div class="flex items-center gap-1.5">
                <h3 class="text-sm font-semibold">Daemon</h3>
                <TooltipProvider v-if="daemonPid" :delay-duration="0">
                  <Tooltip>
                    <TooltipTrigger as-child>
                      <span class="inline-flex cursor-help text-muted-foreground">
                        <Info class="size-3.5" />
                      </span>
                    </TooltipTrigger>
                    <TooltipContent side="top">running as pid {{ daemonPid }}</TooltipContent>
                  </Tooltip>
                </TooltipProvider>
                <StatusPill tone="ok" label="Running" />
              </div>
              <p class="mt-1 text-xs text-muted-foreground">
                <code>yerdd</code> supervises PHP-FPM, serves your
                <code>.{{ tld }}</code> sites, answers DNS, and runs databases.
              </p>
            </div>
            <div class="flex shrink-0 gap-2">
              <Button variant="outline" :disabled="busy !== null" @click="stopDaemonOpen = true">
                <Spinner v-if="busy === 'daemon'" class="size-4" />
                <Square v-else class="size-4" /> Stop
              </Button>
              <Button
                variant="outline"
                :disabled="busy !== null"
                @click="restartDaemonOpen = true"
              >
                <Spinner v-if="busy === 'restart:daemon'" class="size-4" />
                <RotateCw v-else class="size-4" /> Restart
              </Button>
            </div>
          </div>
        </Card>

        <!-- System health summary → Doctor, which owns the fixes for these checks. -->
        <Card>
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <ShieldCheck v-if="!anyUnhealthy" class="size-4 text-success" />
              <ShieldAlert v-else class="size-4 text-warning" />
              <h3 class="text-sm font-semibold">System health</h3>
            </div>
            <RouterLink
              to="/doctor"
              class="text-xs font-medium text-brand transition-colors hover:text-brand/70"
            >
              Open Doctor
            </RouterLink>
          </div>
          <div class="mt-3 grid gap-2 sm:grid-cols-3">
            <div
              v-for="h in health"
              :key="h.key"
              class="flex items-center justify-between rounded-md border px-3 py-2 text-sm"
            >
              <span class="text-muted-foreground">{{ h.label }}</span>
              <StatusPill
                :tone="healthTone(h.value)"
                :label="h.value === true ? h.yes : h.value === false ? h.no : 'Unknown'"
              />
            </div>
          </div>
        </Card>
      </template>
    </div>

    <Modal v-model:open="stopDaemonOpen" title="Stop daemon">
      <p class="text-sm text-muted-foreground">
        This stops all <strong class="text-foreground">.test</strong> sites, DNS,
        and databases until you start Yerd again.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmStopDaemon(close)">Stop</Button>
      </template>
    </Modal>

    <Modal v-model:open="restartDaemonOpen" title="Restart daemon">
      <p class="text-sm text-muted-foreground">
        This briefly stops all <strong class="text-foreground">.test</strong> sites,
        DNS, and this connection while the daemon restarts. It returns in a few
        seconds.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button @click="confirmRestartDaemon(close)">Restart</Button>
      </template>
    </Modal>
  </div>
</template>
