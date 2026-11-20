<script setup lang="ts">
import { Copy, CornerDownRight, ExternalLink, Globe, Trash2, X } from "lucide-vue-next";
import { computed } from "vue";

import SiteDomainsCard from "@/components/sites/SiteDomainsCard.vue";
import Button from "@/components/ui/Button.vue";
import InspectorFooter from "@/components/ui/InspectorFooter.vue";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import Switch from "@/components/ui/Switch.vue";
import { DANGER_TOOL, INSPECTOR_TOOL, SECTION_LABEL } from "@/components/ui/styles";
import { openInBrowser } from "@/ipc/client";
import type { ProxyEntry, StatusReport } from "@/ipc/types";
import { proxyAsSite, proxyHost, type ProxyItem } from "@/lib/proxies";
import { httpsDetail } from "@/lib/sites";
import { isUnbound, openTitle, siteUrl } from "@/lib/siteUrl";
import { copyUrlWithToast, toastOnError } from "@/lib/toastActions";

/**
 * The selected proxy's inspector. A whole-domain proxy shows Open and Copy URL,
 * the ROUTE card (its upstream, read-only, and the HTTPS switch), the DOMAINS
 * card and Remove proxy. A path rule shows its upstream and Remove path. In
 * `panel` mode it sits beside the list; in `drawer` mode it slides over the
 * window (see `InspectorFrame`). Writes go up as events; the view owns them.
 */
const props = defineProps<{
  item: ProxyItem | null;
  mode: "panel" | "drawer";
  /** Drawer mode only: whether the drawer is shown. */
  open?: boolean;
  report: StatusReport | null;
  tld: string;
  busy?: boolean;
}>();

const emit = defineEmits<{
  close: [];
  toggleSecure: [proxy: ProxyEntry];
  remove: [item: ProxyItem];
  domainsChanged: [];
}>();

const drawer = computed(() => props.mode === "drawer");
const shown = computed(() => (drawer.value ? !!props.open : true) && props.item !== null);
const proxy = computed(() => (props.item?.kind === "proxy" ? props.item.proxy : null));
const rule = computed(() => (props.item?.kind === "rule" ? props.item.rule : null));

const url = computed(() => (proxy.value ? siteUrl(proxyAsSite(proxy.value), props.report) : ""));
const unbound = computed(() => isUnbound(props.report));

/** A whole-domain proxy is reachable only through its domain: in unbound mode the
 *  daemon's `localhost/~host` fallback resolves PHP sites only, so Open is gated. */
const openHint = computed(() => {
  const p = proxy.value;
  if (!p) return "";
  return unbound.value
    ? `${proxyHost(p, props.tld)} is reachable only when .${props.tld} DNS resolution is active`
    : openTitle(proxyAsSite(p), props.report);
});

async function openProxy(): Promise<void> {
  await toastOnError("Couldn't open proxy", () => openInBrowser(url.value));
}

async function copyUrl(): Promise<void> {
  await copyUrlWithToast(url.value);
}

const ROW = "flex min-h-14 items-center justify-between gap-4 px-4 py-2.5";
const LABEL = `mb-2 px-1 ${SECTION_LABEL}`;
</script>

<template>
  <InspectorFrame
    :mode="mode"
    :shown="shown"
    label="Proxy details"
    testid="proxy-inspector"
    backdrop-class="proxy-inspector-backdrop"
    @close="emit('close')"
  >
    <div v-if="item" class="flex flex-1 flex-col px-7 pb-6 pt-7">
      <div class="flex items-start gap-3">
        <div class="min-w-0 flex-1">
          <h2
            class="truncate font-mono text-[22px] font-semibold leading-tight tracking-[-0.2px]"
            data-testid="proxy-title"
          >
            <template v-if="proxy">{{ proxyHost(proxy, tld) }}</template>
            <template v-else-if="rule">
              <span class="font-normal text-muted-foreground">{{ rule.site }}.{{ tld }}</span>{{ rule.prefix }}
            </template>
          </h2>
          <p class="mt-1.5 flex items-center gap-1.5 text-xs text-faint" data-testid="proxy-subtitle">
            <template v-if="proxy">
              <Globe class="size-3 shrink-0" aria-hidden="true" />
              <span class="truncate">Whole-domain proxy · every request goes to the upstream</span>
            </template>
            <template v-else-if="rule">
              <CornerDownRight class="size-3 shrink-0" aria-hidden="true" />
              <span class="truncate">Path on a site · PHP serves the rest of {{ rule.site }}.{{ tld }}</span>
            </template>
          </p>
        </div>
        <Button
          v-if="drawer"
          variant="outline"
          size="icon"
          class="size-8 shrink-0"
          aria-label="Close proxy details"
          @click="emit('close')"
        >
          <X />
        </Button>
      </div>

      <div v-if="proxy" class="mt-5 flex flex-wrap items-center gap-2">
        <Button
          :class="INSPECTOR_TOOL"
          :disabled="unbound"
          :title="openHint"
          :aria-label="openHint"
          @click="openProxy"
        >
          <ExternalLink /> Open
        </Button>
        <Button variant="outline" :class="INSPECTOR_TOOL" @click="copyUrl">
          <Copy /> Copy URL
        </Button>
      </div>

      <div class="mt-7 space-y-7">
        <section>
          <h3 :class="LABEL">Route</h3>
          <div class="divide-y rounded-[10px] border bg-card" data-testid="route-card">
            <div :class="ROW">
              <div class="min-w-0">
                <p class="text-[13px] font-semibold">Forward to</p>
                <p class="truncate font-mono text-xs text-muted-foreground" data-testid="forward-to">
                  {{ proxy ? proxy.target : rule?.target }}
                </p>
              </div>
            </div>
            <div v-if="proxy" :class="ROW">
              <div class="min-w-0">
                <p class="text-[13px] font-semibold">HTTPS</p>
                <p class="truncate text-[11px] text-faint" data-testid="https-subtitle">
                  {{ httpsDetail(proxy.secure, report?.ca.trusted_system) }}
                </p>
              </div>
              <Switch
                :model-value="proxy.secure"
                :disabled="busy"
                aria-label="HTTPS"
                @update:model-value="emit('toggleSecure', proxy)"
              />
            </div>
          </div>
        </section>

        <SiteDomainsCard
          v-if="proxy"
          :site="{ name: proxy.name, primary_domain: proxy.primary_domain, domains: proxy.domains }"
          :tld="tld"
          alias-tag
          @domains-changed="emit('domainsChanged')"
        />
      </div>

      <InspectorFooter>
        <Button
          variant="outline"
          :class="DANGER_TOOL"
          :disabled="busy"
          data-testid="inspector-footer-action"
          @click="emit('remove', item)"
        >
          <Trash2 /> {{ proxy ? "Remove proxy…" : "Remove path…" }}
        </Button>
      </InspectorFooter>
    </div>
  </InspectorFrame>
</template>
