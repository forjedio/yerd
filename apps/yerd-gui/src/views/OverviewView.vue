<script setup lang="ts">
import {
  ArrowRight,
  ArrowUpRight,
  Database,
  LayoutGrid,
  Lock,
  LockOpen,
  Mail,
  ShieldAlert,
  ShieldCheck,
  SquareCode,
} from "lucide-vue-next";
import type { Component } from "vue";
import { computed, onMounted, ref, watch } from "vue";
import { RouterLink } from "vue-router";

import DaemonDownHero from "@/components/DaemonDownHero.vue";
import PageHeader from "@/components/PageHeader.vue";
import StatusPill, { type Tone } from "@/components/StatusPill.vue";
import Card from "@/components/ui/Card.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { listSites, openInBrowser } from "@/ipc/client";
import type { Site, StatusReport } from "@/ipc/types";
import { humaniseUptime } from "@/lib/utils";

// The home/dashboard. It reads the shared daemon report (no poller of its own)
// and shows the shared daemon-down hero, so it stays useful when the socket is
// gone — the same surface that celebrates "serving N sites" becomes the start button.
const { report, connected } = useDaemon();

const r = computed(() => report.value);
const running = computed(() => connected.value === true);
// First paint, before the poll has resolved either way.
const connecting = computed(() => connected.value === null && !report.value);
const tld = computed(() => r.value?.tld ?? "test");

// ── live site list (for the console chips) ──
const sites = ref<Site[]>([]);
const sitesLoaded = ref(false);

async function loadSites(): Promise<void> {
  try {
    sites.value = await listSites();
    sitesLoaded.value = true;
  } catch {
    sites.value = []; // a chip strip is non-critical; the headline still shows
  }
}

onMounted(() => {
  if (running.value) loadSites();
});
watch(running, (up) => {
  if (up) {
    loadSites();
  } else {
    sites.value = [];
    sitesLoaded.value = false;
  }
});

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
  // Secured first, then alphabetical — a stable, scannable order.
  return [...sites.value]
    .sort((a, b) => Number(b.secure) - Number(a.secure) || a.name.localeCompare(b.name))
    .slice(0, SITE_CHIP_LIMIT);
});
const moreCount = computed(() =>
  Math.max(0, sites.value.length - sitePreview.value.length),
);

/** Same URL math as the Sites view: scheme + the bound port when it isn't standard. */
function siteUrl(s: Site): string {
  const scheme = s.secure ? "https" : "http";
  const bound = s.secure ? r.value?.https.bound : r.value?.http.bound;
  const dflt = s.secure ? 443 : 80;
  const redirected = r.value?.port_redirect === true;
  const port = !redirected && bound && bound !== dflt ? `:${bound}` : "";
  return `${scheme}://${s.name}.${tld.value}${port}`;
}

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
      sub: x.services.map((s) => s.display_name).join(" · "),
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
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Overview" subtitle="Your local environment at a glance" />

    <div class="flex-1 space-y-4 overflow-y-auto p-6">
      <!-- Connecting: first probe hasn't resolved yet. -->
      <div v-if="connecting" class="flex justify-center py-24">
        <Spinner class="size-6" />
      </div>

      <!-- Daemon down: the shared start affordance (also used on every blocked page). -->
      <DaemonDownHero v-else-if="!running" />

      <!-- Daemon up: the serving console. -->
      <template v-else>
        <Card class="relative overflow-hidden">
          <!-- A soft brand wash — the only place indigo gets a hero surface. -->
          <div
            class="pointer-events-none absolute -right-20 -top-20 size-56 rounded-full bg-brand/5 blur-2xl"
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
                :title="`Open ${s.name}.${tld}`"
                @click="openInBrowser(siteUrl(s))"
              >
                <Lock v-if="s.secure" class="size-3 text-success" />
                <LockOpen v-else class="size-3 text-muted-foreground" />
                <span class="font-mono">{{ s.name }}.{{ tld }}</span>
                <span class="font-mono text-muted-foreground">{{ s.php }}</span>
              </button>
              <RouterLink
                to="/sites"
                class="inline-flex items-center gap-1 rounded-md px-2 py-1 text-xs font-medium text-brand hover:underline"
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

        <!-- Stat tiles → each links to its page. -->
        <div class="grid gap-4 sm:grid-cols-2 lg:grid-cols-4">
          <RouterLink
            v-for="tile in tiles"
            :key="tile.to"
            :to="tile.to"
            class="group rounded-lg border bg-card p-4 shadow-sm transition-colors hover:border-brand/40"
          >
            <div class="flex items-center justify-between">
              <component
                :is="tile.icon"
                class="size-4 text-muted-foreground transition-colors group-hover:text-brand"
              />
              <ArrowUpRight
                class="size-3.5 text-transparent transition-colors group-hover:text-muted-foreground"
              />
            </div>
            <p class="mt-3 text-2xl font-semibold tracking-tight">
              {{ tile.value }}
              <span class="ml-0.5 text-sm font-normal text-muted-foreground">
                {{ tile.unit }}
              </span>
            </p>
            <p class="mt-0.5 text-xs font-medium">{{ tile.label }}</p>
            <p class="mt-1 truncate text-xs text-muted-foreground" :title="tile.sub">
              {{ tile.sub }}
            </p>
          </RouterLink>
        </div>

        <!-- System health summary → Settings. -->
        <Card>
          <div class="flex items-center justify-between">
            <div class="flex items-center gap-2">
              <ShieldCheck v-if="!anyUnhealthy" class="size-4 text-success" />
              <ShieldAlert v-else class="size-4 text-warning" />
              <h3 class="text-sm font-semibold">System health</h3>
            </div>
            <RouterLink
              to="/general"
              class="text-xs font-medium text-brand hover:underline"
            >
              Open Settings
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
  </div>
</template>
