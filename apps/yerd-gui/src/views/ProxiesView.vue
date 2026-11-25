<script setup lang="ts">
import { computed, nextTick, onUnmounted, ref, watch } from "vue";
import { Plus, Trash2 } from "lucide-vue-next";

import EditUpstreamDialog from "@/components/proxies/EditUpstreamDialog.vue";
import NewProxyDialog, {
  type AddPathRequest,
  type AddProxyRequest,
  type NewProxyMode,
} from "@/components/proxies/NewProxyDialog.vue";
import ProxiesEmptyState from "@/components/proxies/ProxiesEmptyState.vue";
import ProxiesList from "@/components/proxies/ProxiesList.vue";
import ProxyInspector, { type ProxyNameState } from "@/components/proxies/ProxyInspector.vue";
import SitesWarningStrip from "@/components/sites/SitesWarningStrip.vue";
import PageHeader from "@/components/PageHeader.vue";
import AsyncState from "@/components/ui/AsyncState.vue";
import Button from "@/components/ui/Button.vue";
import ConfirmDialog from "@/components/ui/ConfirmDialog.vue";
import FilterChips from "@/components/ui/FilterChips.vue";
import { useDaemon } from "@/composables/useDaemon";
import { usePoll } from "@/composables/usePoll";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { useWideLayout } from "@/composables/useWideLayout";
import {
  addProxy,
  addProxyRule,
  IpcError,
  listProxies,
  removeProxy,
  removeProxyRule,
  setSecure,
  sitesAndParked,
  updateProxy,
  updateProxyRule,
} from "@/ipc/client";
import type { ProxyEntry, ProxyRuleEntry, SiteEntry } from "@/ipc/types";
import {
  proxyChips,
  type ProxyChipKey,
  proxyHost,
  type ProxyItem,
  proxyKey,
  proxySections,
  ruleKey,
  upstreamLabel,
} from "@/lib/proxies";
import type { SiteWarning } from "@/lib/sites";
import { registerViewActions } from "@/lib/shortcuts/useViewActions";

const toast = useToast();
const { report } = useDaemon();
const wide = useWideLayout();

const {
  data: proxyData,
  loading,
  error: resourceError,
  refresh: load,
} = useResource("proxies", listProxies);

// The site picker for path rules shares the Sites view's cached resource (same
// key + fetcher reference), so it stays coherent with everything else.
const {
  data: sitesData,
  error: sitesError,
  refresh: loadSites,
} = useResource("sites", sitesAndParked);
usePoll(() => Promise.all([load(), loadSites()]), 5000);

const proxies = computed<ProxyEntry[]>(() => proxyData.value?.proxies ?? []);
const rules = computed<ProxyRuleEntry[]>(() => proxyData.value?.rules ?? []);
const sites = computed<SiteEntry[]>(() => sitesData.value?.sites ?? []);
// Surface a load failure only when nothing is cached to show; a failed
// background revalidation keeps the last-good list.
const error = computed(() =>
  proxyData.value ? null : (resourceError.value?.message ?? null),
);
const isEmpty = computed(() => proxies.value.length === 0 && rules.value.length === 0);

const tld = computed(() => report.value?.tld ?? "test");
const hasSecureProxy = computed(() => proxies.value.some((p) => p.secure));

const caWarning = computed<SiteWarning | null>(() => {
  if (!report.value || !hasSecureProxy.value || report.value.ca.trusted_system !== false) return null;
  const text = "Browsers will warn on HTTPS proxies until you trust Yerd's local certificate authority.";
  return { key: "ca", text, compactText: text, action: "Trust now" };
});

// Per-row busy keys (`proxy:<name>` / `rule:<site><prefix>`), so a mutation on
// one row doesn't clear another's in-flight spinner when they overlap.
const busyKeys = ref(new Set<string>());
function setBusy(key: string, busy: boolean): void {
  if (busy) busyKeys.value.add(key);
  else busyKeys.value.delete(key);
}
function isBusy(key: string): boolean {
  return busyKeys.value.has(key);
}

// ── filtering and selection ──
const activeChip = ref<ProxyChipKey>("all");
const chips = computed(() => proxyChips(proxies.value, rules.value));
const sections = computed(() => proxySections(proxies.value, rules.value, activeChip.value));
const items = computed(() => sections.value.flatMap((s) => s.items));

const selectedKey = ref<string | null>(null);
const drawerOpen = ref(false);
const selectedItem = computed<ProxyItem | null>(
  () => items.value.find((i) => i.key === selectedKey.value) ?? null,
);

watch(
  [items, wide],
  ([list, isWide]) => {
    if (!isWide) return;
    if (!selectedKey.value || !list.some((i) => i.key === selectedKey.value)) {
      selectedKey.value = list[0]?.key ?? null;
    }
  },
  { immediate: true },
);

watch(selectedItem, (item) => {
  if (!item) drawerOpen.value = false;
});

watch(wide, (isWide) => {
  if (isWide) drawerOpen.value = false;
});

function selectItem(key: string): void {
  selectedKey.value = key;
}

function activateItem(key: string): void {
  selectedKey.value = key;
  if (!wide.value) drawerOpen.value = true;
}

// ── HTTPS toggle (reuses the daemon's SetSecure, which handles proxies) ──
async function toggleSecure(p: ProxyEntry): Promise<void> {
  const next = !p.secure;
  const key = proxyKey(p.name);
  setBusy(key, true);
  try {
    await setSecure(p.name, next);
    toast.success(
      next
        ? `HTTPS enabled for ${proxyHost(p, tld.value)}`
        : `HTTPS disabled for ${proxyHost(p, tld.value)}`,
    );
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't change HTTPS", (e as IpcError).message);
  } finally {
    setBusy(key, false);
  }
}

// ── new proxy / path ──
const newOpen = ref(false);
const newMode = ref<NewProxyMode | undefined>(undefined);

/** Open New proxy, optionally in a given mode. Deferred a tick so a closing
 *  menu's focus restore doesn't fight the dialog. */
function openNew(mode?: NewProxyMode): void {
  newMode.value = mode;
  void nextTick(() => {
    newOpen.value = true;
  });
}

async function confirmAddProxy({ name, url, secure }: AddProxyRequest): Promise<void> {
  const host = `${name}.${tld.value}`;
  const key = proxyKey(name);
  setBusy(key, true);
  let created = false;
  try {
    await addProxy(name, url);
    created = true;
    if (secure) await setSecure(name, true);
    toast.success(`Added ${host}`, `Forwarding to ${upstreamLabel(url)}`);
  } catch (e) {
    const msg = (e as IpcError).message;
    if (created) {
      toast.error(`Added ${host} on HTTP only`, `Couldn't enable HTTPS: ${msg}`);
    } else {
      toast.error("Couldn't add proxy", msg);
    }
  } finally {
    await load({ force: true });
    setBusy(key, false);
  }
}

async function confirmAddRule({ site, prefix, url }: AddPathRequest): Promise<void> {
  const key = ruleKey(site, prefix);
  setBusy(key, true);
  try {
    await addProxyRule(site, prefix, url);
    toast.success(`Added ${site}.${tld.value}${prefix}`, `Forwarding to ${upstreamLabel(url)}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't add path", (e as IpcError).message);
  } finally {
    setBusy(key, false);
  }
}

// ── edit upstream ──
// Like remove, the dialog's target is captured at open time and never cleared,
// so a save always updates the item whose Edit was clicked.
const editOpen = ref(false);
const editTarget = ref<ProxyItem | null>(null);

function openEdit(item: ProxyItem): void {
  editTarget.value = item;
  void nextTick(() => {
    editOpen.value = true;
  });
}

const editHost = computed(() => {
  const t = editTarget.value;
  if (!t) return "";
  return t.kind === "proxy" ? proxyHost(t.proxy, tld.value) : `${t.rule.site}.${tld.value}${t.rule.prefix}`;
});

const editCurrent = computed(() => {
  const t = editTarget.value;
  if (!t) return "";
  return t.kind === "proxy" ? t.proxy.target : t.rule.target;
});

async function confirmEditProxy(p: ProxyEntry, url: string): Promise<void> {
  const key = proxyKey(p.name);
  setBusy(key, true);
  try {
    await updateProxy(p.name, url);
    toast.success(`Updated ${proxyHost(p, tld.value)}`, `Forwarding to ${upstreamLabel(url)}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't update proxy", (e as IpcError).message);
  } finally {
    setBusy(key, false);
  }
}

async function confirmEditRule(r: ProxyRuleEntry, url: string): Promise<void> {
  const key = ruleKey(r.site, r.prefix);
  setBusy(key, true);
  try {
    await updateProxyRule(r.site, r.prefix, url);
    toast.success(`Updated ${r.site}.${tld.value}${r.prefix}`, `Forwarding to ${upstreamLabel(url)}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't update path", (e as IpcError).message);
  } finally {
    setBusy(key, false);
  }
}

function confirmEdit(url: string): void {
  const t = editTarget.value;
  if (!t) return;
  if (t.kind === "proxy") void confirmEditProxy(t.proxy, url);
  else void confirmEditRule(t.rule, url);
}

function onDomainsChanged(): void {
  void load({ force: true });
}

// ── remove proxy / path ──
// The confirm hands its target to the remove call when clicked, and the
// target is never cleared afterwards, so a second confirm opened while the
// first removal runs keeps its own target.
const removeOpen = ref(false);
const removeTarget = ref<ProxyItem | null>(null);

function openRemove(item: ProxyItem): void {
  removeTarget.value = item;
  void nextTick(() => {
    removeOpen.value = true;
  });
}

const removeCopy = computed(() => {
  const t = removeTarget.value;
  if (!t) return { title: "", body: "", label: "Remove" };
  if (t.kind === "proxy") {
    return {
      title: `Remove ${proxyHost(t.proxy, tld.value)}?`,
      body: `Yerd stops serving this domain. Whatever runs on ${upstreamLabel(t.proxy.target)} keeps running.`,
      label: "Remove proxy",
    };
  }
  return {
    title: `Remove ${t.rule.site}.${tld.value}${t.rule.prefix}?`,
    body: `That path goes back to PHP. Whatever runs on ${upstreamLabel(t.rule.target)} keeps running.`,
    label: "Remove path",
  };
});

async function confirmRemoveProxy(p: ProxyEntry): Promise<void> {
  const key = proxyKey(p.name);
  setBusy(key, true);
  try {
    await removeProxy(p.name);
    toast.success(`Removed ${proxyHost(p, tld.value)}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't remove proxy", (e as IpcError).message);
  } finally {
    setBusy(key, false);
  }
}

async function confirmRemoveRule(r: ProxyRuleEntry): Promise<void> {
  const key = ruleKey(r.site, r.prefix);
  setBusy(key, true);
  try {
    await removeProxyRule(r.site, r.prefix);
    toast.success(`Removed ${r.site}.${tld.value}${r.prefix}`);
    await load({ force: true });
  } catch (e) {
    toast.error("Couldn't remove path", (e as IpcError).message);
  } finally {
    setBusy(key, false);
  }
}

function confirmRemove(): void {
  const t = removeTarget.value;
  if (!t) return;
  if (t.kind === "proxy") void confirmRemoveProxy(t.proxy);
  else void confirmRemoveRule(t.rule);
}

onUnmounted(
  registerViewActions({
    create: () => openNew(),
    refresh: () => void load(),
  }),
);

/** A site with the proxy's name takes the name over, so the proxy is not routed.
 *  Until the site list has loaded without error the answer is `unknown`. */
function proxyNameState(item: ProxyItem | null): ProxyNameState {
  if (item?.kind !== "proxy") return "free";
  if (!sitesData.value || sitesError.value) return "unknown";
  return sites.value.some((s) => s.name === item.proxy.name) ? "taken" : "free";
}

const inspectorProps = computed(() => ({
  item: selectedItem.value,
  report: report.value ?? null,
  tld: tld.value,
  busy: selectedItem.value ? isBusy(selectedItem.value.key) : false,
  nameState: proxyNameState(selectedItem.value),
}));
</script>

<template>
  <div class="flex h-full flex-col">
    <PageHeader title="Proxies" docs="/guide/proxies">
      <template #actions>
        <Button size="sm" @click="openNew()"><Plus /> New proxy</Button>
      </template>
    </PageHeader>

    <SitesWarningStrip v-if="caWarning" :warning="caWarning" :compact="!wide" />

    <div class="min-h-0 flex-1">
      <AsyncState :loading="loading" :error="error" :empty="isEmpty" pad="py-16" @retry="load">
        <template #empty>
          <ProxiesEmptyState :tld="tld" :has-sites="sites.length > 0" @create="openNew" />
        </template>

        <div class="flex h-full min-h-0" :data-layout="wide ? 'wide' : 'compact'">
          <div class="flex min-w-0 flex-1 flex-col pl-4 pt-4">
            <div class="pr-4">
              <FilterChips v-model="activeChip" :chips="chips" label="Filter proxies" />
            </div>

            <ProxiesList
              class="mt-1 flex-1"
              :sections="sections"
              :layout="wide ? 'list' : 'table'"
              :selected="selectedKey"
              :tld="tld"
              :sites="sites"
              :busy="isBusy"
              @select="selectItem"
              @activate="activateItem"
            />
          </div>

          <div v-if="wide" class="w-[58%] max-w-[760px] shrink-0 border-l" data-testid="inspector-column">
            <ProxyInspector
              v-if="selectedItem"
              mode="panel"
              v-bind="inspectorProps"
              @toggle-secure="toggleSecure"
              @remove="openRemove"
              @edit="openEdit"
              @domains-changed="onDomainsChanged"
            />
            <p v-else class="flex h-full items-center justify-center text-[13px] text-faint">
              Select a proxy to see its details.
            </p>
          </div>
        </div>
      </AsyncState>
    </div>

    <ProxyInspector
      v-if="!wide"
      mode="drawer"
      :open="drawerOpen && selectedItem !== null"
      v-bind="inspectorProps"
      @close="drawerOpen = false"
      @toggle-secure="toggleSecure"
      @remove="openRemove"
      @edit="openEdit"
      @domains-changed="onDomainsChanged"
    />

    <NewProxyDialog
      v-model:open="newOpen"
      :initial-mode="newMode"
      :tld="tld"
      :sites="sites"
      :trusted="report?.ca.trusted_system ?? null"
      @add-proxy="confirmAddProxy"
      @add-path="confirmAddRule"
    />

    <EditUpstreamDialog
      v-model:open="editOpen"
      :host="editHost"
      :current="editCurrent"
      @save="confirmEdit"
    />

    <ConfirmDialog
      v-model:open="removeOpen"
      :title="removeCopy.title"
      :confirm-label="removeCopy.label"
      tone="danger"
      :icon="Trash2"
      @confirm="confirmRemove"
    >
      {{ removeCopy.body }}
    </ConfirmDialog>
  </div>
</template>
