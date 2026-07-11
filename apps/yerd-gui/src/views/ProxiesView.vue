<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref } from "vue";
import {
  ChevronDown,
  ExternalLink,
  Lock,
  LockOpen,
  Plus,
  Route,
  ShieldAlert,
  Trash2,
  Waypoints,
} from "lucide-vue-next";

import PageHeader from "@/components/PageHeader.vue";
import AsyncState from "@/components/ui/AsyncState.vue";
import Button from "@/components/ui/Button.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import EmptyState from "@/components/ui/EmptyState.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { useDaemon } from "@/composables/useDaemon";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { isUnbound, openTitle, siteUrl } from "@/lib/siteUrl";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";
import {
  addProxy,
  addProxyRule,
  IpcError,
  listProxies,
  openInBrowser,
  removeProxy,
  removeProxyRule,
  setSecure,
  sitesAndParked,
} from "@/ipc/client";
import type { ProxyEntry, ProxyRuleEntry } from "@/ipc/types";

const toast = useToast();
const { report } = useDaemon();

const {
  data: proxyData,
  loading,
  error: resourceError,
  refresh: load,
} = useResource("proxies", listProxies);
usePoll(() => load(), 5000);

// The site picker for path rules shares the Sites view's cached resource (same
// key + fetcher reference), so it stays coherent with everything else.
const { data: sitesData } = useResource("sites", sitesAndParked);

const proxies = computed<ProxyEntry[]>(() =>
  [...(proxyData.value?.proxies ?? [])].sort((a, b) => a.name.localeCompare(b.name)),
);
const rules = computed<ProxyRuleEntry[]>(() => proxyData.value?.rules ?? []);
// Surface a load failure only when nothing is cached to show; a failed
// background revalidation keeps the last-good list.
const error = computed(() =>
  proxyData.value ? null : (resourceError.value?.message ?? null),
);
const isEmpty = computed(() => proxies.value.length === 0 && rules.value.length === 0);

const tld = computed(() => report.value?.tld ?? "test");
const caTrusted = computed(() => report.value?.ca.trusted_system === true);
const hasSecureProxy = computed(() => proxies.value.some((p) => p.secure));

const rowBusy = ref<string | null>(null);

/** Path rules grouped by site, each group's rules sorted by prefix. */
const ruleGroups = computed(() => {
  const by = new Map<string, ProxyRuleEntry[]>();
  for (const r of rules.value) {
    const list = by.get(r.site) ?? [];
    list.push(r);
    by.set(r.site, list);
  }
  return [...by.entries()]
    .map(([site, list]) => ({
      site,
      rules: [...list].sort((a, b) => a.prefix.localeCompare(b.prefix)),
    }))
    .sort((a, b) => a.site.localeCompare(b.site));
});

const siteOptions = computed(() =>
  (sitesData.value?.sites ?? [])
    .map((s) => s.name)
    .sort((a, b) => a.localeCompare(b))
    .map((name) => ({ value: name, label: `${name}.${tld.value}` })),
);
const hasSites = computed(() => siteOptions.value.length > 0);

/** A `SiteLike` for the shared URL helpers - a whole-host proxy is served on the
 *  same ports as sites, so it obeys the same bound-port / unbound rules. */
function proxyAsSite(p: ProxyEntry): { name: string; secure: boolean } {
  return { name: p.name, secure: p.secure };
}

// A whole-host proxy is reachable only via its .test domain: in resolver-off
// (unbound) mode the daemon's `localhost/~host` fallback resolves PHP sites only,
// so a proxy has no working URL there. Gate its Open affordance accordingly.
const unbound = computed(() => isUnbound(report.value));

function openProxy(p: ProxyEntry): void {
  void openInBrowser(siteUrl(proxyAsSite(p), report.value));
}

function proxyOpenTitle(p: ProxyEntry): string {
  return unbound.value
    ? `${p.name}.${tld.value} is reachable only when .test DNS resolution is active`
    : openTitle(proxyAsSite(p), report.value);
}

/** Coerce user upstream input into what the daemon's parser accepts: default the
 *  scheme to `http://`, then strip a trailing slash so a pasted
 *  `http://host:port/` isn't rejected as a path. Strips only when a host follows
 *  the scheme, leaving a bare `http://` intact (rejected honestly as no host). */
function normalizeUpstream(raw: string): string {
  let u = raw.trim();
  if (u !== "" && !/^https?:\/\//i.test(u)) u = `http://${u}`;
  return u.replace(/^(https?:\/\/.+?)\/+$/i, "$1");
}

// ── HTTPS toggle (reuses the daemon's SetSecure, which handles proxies) ──
async function toggleSecure(p: ProxyEntry): Promise<void> {
  const next = !p.secure;
  rowBusy.value = `proxy:${p.name}`;
  try {
    await setSecure(p.name, next);
    toast.success(
      next
        ? `HTTPS enabled for ${p.name}.${tld.value}`
        : `HTTPS disabled for ${p.name}.${tld.value}`,
    );
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change HTTPS", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

// ── new whole-host proxy ──
const addProxyOpen = ref(false);
const newProxyName = ref("");
const newProxyUrl = ref("");
const newProxySecure = ref(false);
const newProxyValid = computed(
  () => /^[a-z0-9-]+$/i.test(newProxyName.value.trim()) && newProxyUrl.value.trim() !== "",
);

function openAddProxy(): void {
  newProxyName.value = "";
  newProxyUrl.value = "";
  newProxySecure.value = false;
  void nextTick(() => {
    addProxyOpen.value = true;
  });
}

async function confirmAddProxy(close: () => void): Promise<void> {
  const name = newProxyName.value.trim();
  const url = normalizeUpstream(newProxyUrl.value);
  const secure = newProxySecure.value;
  const wasValid = newProxyValid.value;
  close();
  if (!name || !url || !wasValid) return;
  rowBusy.value = `proxy:${name}`;
  let created = false;
  try {
    await addProxy(name, url);
    created = true;
    if (secure) await setSecure(name, true);
    toast.success(`Added proxy ${name}.${tld.value}`);
  } catch (e) {
    const msg = (e as IpcError).message;
    if (created) {
      toast.error(`Added ${name}.${tld.value} on HTTP`, `Couldn't enable HTTPS: ${msg}`);
    } else {
      toast.error("Couldn't add proxy", msg);
    }
  } finally {
    await load({ force: true });
    rowBusy.value = null;
  }
}

// ── new path rule ──
const addRuleOpen = ref(false);
const newRuleSite = ref("");
const newRulePrefix = ref("");
const newRuleUrl = ref("");
const newRuleValid = computed(
  () =>
    newRuleSite.value !== "" &&
    newRulePrefix.value.trim().startsWith("/") &&
    newRuleUrl.value.trim() !== "",
);

function openAddRule(): void {
  newRuleSite.value = siteOptions.value[0]?.value ?? "";
  newRulePrefix.value = "";
  newRuleUrl.value = "";
  void nextTick(() => {
    addRuleOpen.value = true;
  });
}

async function confirmAddRule(close: () => void): Promise<void> {
  const site = newRuleSite.value;
  const prefix = newRulePrefix.value.trim();
  const url = normalizeUpstream(newRuleUrl.value);
  const wasValid = newRuleValid.value;
  close();
  if (!site || !prefix || !url || !wasValid) return;
  rowBusy.value = `rule:${site}${prefix}`;
  try {
    await addProxyRule(site, prefix, url);
    toast.success(`Added rule ${site}.${tld.value}${prefix}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't add rule", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
  }
}

// ── remove proxy ──
const removeProxyOpen = ref(false);
const removeProxyTarget = ref<ProxyEntry | null>(null);

function openRemoveProxy(p: ProxyEntry): void {
  removeProxyTarget.value = p;
  void nextTick(() => {
    removeProxyOpen.value = true;
  });
}

async function confirmRemoveProxy(close: () => void): Promise<void> {
  const p = removeProxyTarget.value;
  close();
  if (!p) return;
  rowBusy.value = `proxy:${p.name}`;
  try {
    await removeProxy(p.name);
    toast.success(`Removed proxy ${p.name}.${tld.value}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't remove proxy", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
    removeProxyTarget.value = null;
  }
}

// ── remove rule ──
const removeRuleOpen = ref(false);
const removeRuleTarget = ref<ProxyRuleEntry | null>(null);

function openRemoveRule(r: ProxyRuleEntry): void {
  removeRuleTarget.value = r;
  void nextTick(() => {
    removeRuleOpen.value = true;
  });
}

async function confirmRemoveRule(close: () => void): Promise<void> {
  const r = removeRuleTarget.value;
  close();
  if (!r) return;
  rowBusy.value = `rule:${r.site}${r.prefix}`;
  try {
    await removeProxyRule(r.site, r.prefix);
    toast.success(`Removed rule ${r.site}.${tld.value}${r.prefix}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't remove rule", (e as IpcError).message);
  } finally {
    rowBusy.value = null;
    removeRuleTarget.value = null;
  }
}

onUnmounted(
  registerViewActions({
    create: openAddProxy,
    refresh: () => void load(),
  }),
);
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader
      title="Proxies"
      subtitle="Reverse-proxy .test domains and site paths to local upstreams"
      docs="/guide/proxies"
    >
      <template #actions>
        <DropdownMenu>
          <DropdownMenuTrigger as-child>
            <Button>
              <Plus class="size-4" /> New <ChevronDown class="size-3.5 opacity-70" />
            </Button>
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-56">
            <DropdownMenuItem @select="openAddProxy">
              <Waypoints class="size-4" /> New proxy…
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem :disabled="!hasSites" @select="openAddRule">
              <Route class="size-4" /> New path rule…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </template>
    </PageHeader>

    <div class="flex-1 overflow-y-auto p-6">
      <!-- CA-not-trusted warning (proxies still serve, browsers just warn). -->
      <div
        v-if="hasSecureProxy && !caTrusted && report"
        class="mb-4 flex items-start gap-2 rounded-md border border-warning/40 bg-warning/10 p-3 text-xs"
      >
        <ShieldAlert class="mt-0.5 size-4 shrink-0 text-warning" />
        <span>
          The local CA isn't trusted in your system store, so browsers will warn
          on HTTPS proxies. Fix it under
          <RouterLink to="/doctor" class="font-medium underline">Doctor → Environment</RouterLink>.
        </span>
      </div>

      <AsyncState :loading="loading" :error="error" :empty="isEmpty" pad="py-16" @retry="load">
        <template #empty>
          <EmptyState
            :icon="Waypoints"
            title="No proxies yet"
            description="Point a .test domain at any local upstream (e.g. a Docker container on localhost:9011), or route a path of an existing site to an upstream while PHP serves the rest."
          >
            <div class="flex gap-2">
              <Button @click="openAddProxy"><Waypoints class="size-4" /> New proxy</Button>
              <Button variant="outline" :disabled="!hasSites" @click="openAddRule">
                <Route class="size-4" /> New path rule
              </Button>
            </div>
          </EmptyState>
        </template>

        <!-- Whole-host proxies -->
        <section v-if="proxies.length">
          <h3 class="mb-2 text-sm font-semibold">Whole-host proxies</h3>
          <div class="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
            <div
              v-for="p in proxies"
              :key="p.name"
              class="group rounded-lg border bg-card p-4 shadow-sm transition-colors hover:border-brand/40"
            >
              <div class="flex items-start justify-between gap-2">
                <button
                  class="flex min-w-0 items-center gap-1.5 font-mono text-sm font-medium hover:text-brand disabled:cursor-default disabled:hover:text-foreground"
                  :disabled="unbound"
                  :title="proxyOpenTitle(p)"
                  @click="openProxy(p)"
                >
                  <span class="truncate">{{ p.name }}.{{ tld }}</span>
                </button>
                <div class="flex shrink-0 items-center">
                  <Spinner v-if="rowBusy === `proxy:${p.name}`" class="size-4" />
                  <template v-else>
                    <Button
                      variant="ghost"
                      size="icon"
                      :disabled="unbound"
                      :aria-label="proxyOpenTitle(p)"
                      :title="proxyOpenTitle(p)"
                      @click="openProxy(p)"
                    >
                      <ExternalLink class="size-4" />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon"
                      :aria-label="`Remove proxy ${p.name}.${tld}`"
                      title="Remove proxy"
                      @click="openRemoveProxy(p)"
                    >
                      <Trash2 class="size-4" />
                    </Button>
                  </template>
                </div>
              </div>

              <p class="mt-1 truncate font-mono text-xs text-muted-foreground" :title="p.target">
                → {{ p.target }}
              </p>

              <div class="mt-3 flex items-center gap-1.5">
                <button
                  type="button"
                  :disabled="rowBusy === `proxy:${p.name}`"
                  :aria-label="p.secure ? 'Serve over HTTP' : 'Serve over HTTPS'"
                  :title="
                    p.secure
                      ? 'Serving over HTTPS - click to switch to HTTP'
                      : 'Serving over HTTP - click to switch to HTTPS'
                  "
                  class="inline-flex items-center gap-1 rounded-md px-1.5 py-0.5 text-[11px] font-medium transition-opacity hover:opacity-70 disabled:cursor-not-allowed disabled:opacity-50"
                  :class="p.secure ? 'bg-success/10 text-success' : 'bg-muted text-muted-foreground'"
                  @click="toggleSecure(p)"
                >
                  <Lock v-if="p.secure" class="size-3" />
                  <LockOpen v-else class="size-3" />
                  {{ p.secure ? "HTTPS" : "HTTP" }}
                </button>
              </div>
            </div>
          </div>
        </section>

        <!-- Per-site path rules -->
        <section v-if="ruleGroups.length" :class="proxies.length ? 'mt-8' : ''">
          <div class="mb-2">
            <h3 class="text-sm font-semibold">Path rules</h3>
            <p class="text-xs text-muted-foreground">
              A path prefix of a
              <RouterLink to="/sites" class="underline">site</RouterLink>
              routes to an upstream; every other path is served by PHP.
            </p>
          </div>
          <div class="space-y-3">
            <div v-for="g in ruleGroups" :key="g.site" class="rounded-lg border">
              <div class="border-b px-3 py-2 text-sm font-medium">{{ g.site }}.{{ tld }}</div>
              <div class="divide-y">
                <div
                  v-for="r in g.rules"
                  :key="r.prefix"
                  class="flex items-center justify-between gap-3 px-3 py-2.5"
                >
                  <div class="min-w-0 font-mono text-xs">
                    <span class="text-foreground">{{ r.prefix }}</span>
                    <span class="text-muted-foreground"> → {{ r.target }}</span>
                  </div>
                  <Spinner v-if="rowBusy === `rule:${r.site}${r.prefix}`" class="size-4 shrink-0" />
                  <Button
                    v-else
                    variant="ghost"
                    size="icon"
                    :aria-label="`Remove rule ${r.site}.${tld}${r.prefix}`"
                    title="Remove rule"
                    @click="openRemoveRule(r)"
                  >
                    <Trash2 class="size-4" />
                  </Button>
                </div>
              </div>
            </div>
          </div>
        </section>
      </AsyncState>
    </div>

    <!-- new whole-host proxy -->
    <Modal v-model:open="addProxyOpen" title="New proxy">
      <div class="space-y-4">
        <div>
          <label class="text-sm font-medium" for="proxyname">Name (single label)</label>
          <Input id="proxyname" v-model="newProxyName" placeholder="e.g. mydockersite" class="mt-2" />
          <p class="mt-1 text-xs text-muted-foreground">
            Served at <code class="font-mono">{{ (newProxyName.trim() || "name") }}.{{ tld }}</code>.
          </p>
        </div>
        <div>
          <label class="text-sm font-medium" for="proxyurl">Upstream URL</label>
          <Input id="proxyurl" v-model="newProxyUrl" placeholder="http://localhost:9011" class="mt-2" />
          <p class="mt-1 text-xs text-muted-foreground">
            The address to forward every request to. Assumes
            <code class="font-mono">http://</code> if you omit the scheme.
          </p>
        </div>
        <div class="flex items-center justify-between gap-4">
          <div>
            <p class="text-sm font-medium">HTTPS</p>
            <p class="text-xs text-muted-foreground">Serve this proxy over TLS.</p>
          </div>
          <Switch v-model="newProxySecure" aria-label="HTTPS" />
        </div>
      </div>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button :disabled="!newProxyValid" @click="confirmAddProxy(close)">Add proxy</Button>
      </template>
    </Modal>

    <!-- new path rule -->
    <Modal v-model:open="addRuleOpen" title="New path rule">
      <div class="space-y-4">
        <div>
          <label class="text-sm font-medium" for="rulesite">Site</label>
          <div class="mt-2">
            <Select
              id="rulesite"
              :model-value="newRuleSite"
              :options="siteOptions"
              class="w-full"
              aria-label="Site"
              @update:model-value="(v: string) => (newRuleSite = v)"
            />
          </div>
        </div>
        <div>
          <label class="text-sm font-medium" for="ruleprefix">Path prefix</label>
          <Input id="ruleprefix" v-model="newRulePrefix" placeholder="/app" class="mt-2" />
          <p class="mt-1 text-xs text-muted-foreground">
            Requests under this prefix proxy to the upstream; the rest is served by
            PHP. Must begin with <code class="font-mono">/</code>.
          </p>
        </div>
        <div>
          <label class="text-sm font-medium" for="ruleurl">Upstream URL</label>
          <Input id="ruleurl" v-model="newRuleUrl" placeholder="http://localhost:9011" class="mt-2" />
        </div>
      </div>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button :disabled="!newRuleValid" @click="confirmAddRule(close)">Add rule</Button>
      </template>
    </Modal>

    <!-- remove proxy confirm -->
    <Modal
      v-model:open="removeProxyOpen"
      title="Remove proxy"
      @update:open="(v: boolean) => { if (!v) removeProxyTarget = null; }"
    >
      <p class="text-sm text-muted-foreground">
        Remove <strong class="text-foreground">{{ removeProxyTarget?.name }}.{{ tld }}</strong>?
        That host stops being served.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button variant="destructive" @click="confirmRemoveProxy(close)">Remove</Button>
      </template>
    </Modal>

    <!-- remove rule confirm -->
    <Modal
      v-model:open="removeRuleOpen"
      title="Remove path rule"
      @update:open="(v: boolean) => { if (!v) removeRuleTarget = null; }"
    >
      <p class="text-sm text-muted-foreground">
        Remove the rule
        <strong class="text-foreground font-mono"
          >{{ removeRuleTarget?.site }}.{{ tld }}{{ removeRuleTarget?.prefix }}</strong
        >? That path returns to being served by PHP.
      </p>
      <template #footer="{ close }">
        <Button variant="ghost" @click="close">Cancel</Button>
        <Button variant="destructive" @click="confirmRemoveRule(close)">Remove</Button>
      </template>
    </Modal>
  </div>
</template>
