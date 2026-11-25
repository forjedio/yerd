<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Globe, KeyRound, Link, Lock } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Combobox from "@/components/ui/Combobox.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { Site, TunnelProvider, TunnelProviderStatus } from "@/ipc/types";
import { PROVIDER_ORDER, providerName, readinessDot, readinessLabel } from "@/lib/providers";
import {
  hostPlaceholder,
  isShareHostname,
  ngrokDomainHolder,
  scheme,
  type ShareItem,
  type ShareKind,
  suggestedHost,
} from "@/lib/share";
import { cn, joinWithAnd } from "@/lib/utils";

import ProviderTile from "./providers/ProviderTile.vue";

/** What the dialog asks the view to share. `hostname` is set for a domain
 *  share; `randomUrl` asks an ngrok share to ignore the static domain. */
export interface ShareRequest {
  site: string;
  provider: TunnelProvider;
  kind: ShareKind;
  hostname: string | null;
  randomUrl: boolean;
}

/**
 * The one-step Share a site dialog: pick a site, a provider, then how its URL
 * is chosen. Cloudflare offers a Quick link (a temporary trycloudflare.com URL)
 * or Your domain (a hostname on the account's zone through the domain tunnel,
 * which follows the site until edited). ngrok offers its static domain, when
 * one is configured and free, or a random URL. zrok gives a new public URL
 * each start. A provider that is not ready replaces the URL choice with a
 * "Set up" call to action. While the share is in flight the dialog stays open
 * and locked; the view closes it on success.
 */
const props = defineProps<{
  open: boolean;
  sites: Site[];
  /** Every current share, for the taken sites and the already-shared note. */
  items: ShareItem[];
  providers: TunnelProviderStatus[];
  defaultProvider: TunnelProvider;
  initialSite?: string | null;
  initialProvider?: TunnelProvider | null;
  zone: string | null;
  /** Whether a Cloudflare account is connected and a tunnel exists. */
  domainReady: boolean;
  tld: string;
  /** A share from this dialog is in flight. */
  busy: boolean;
  /** Another action is in flight or the daemon is down. */
  locked: boolean;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  share: [request: ShareRequest];
  setup: [provider: TunnelProvider];
}>();

const provider = ref<TunnelProvider>("cloudflare");
const kind = ref<ShareKind>("quick");
const site = ref<string | null>(null);
const hostname = ref("");
const hostEdited = ref(false);
const urlChoice = ref<"static" | "random">("random");

const statusOf = (p: TunnelProvider) => props.providers.find((s) => s.provider === p) ?? null;
const ready = (p: TunnelProvider) => statusOf(p)?.readiness === "ready";
const providerReady = computed(() => ready(provider.value));
const ngrokDomain = computed(() => statusOf("ngrok")?.domain ?? null);
const domainHolder = computed(() => ngrokDomainHolder(props.items, ngrokDomain.value));
const staticFree = computed(() => !!ngrokDomain.value && domainHolder.value === null);

function resetUrlChoice(): void {
  urlChoice.value = staticFree.value ? "static" : "random";
}

watch(
  () => props.open,
  (open) => {
    if (!open) return;
    provider.value = props.initialProvider ?? props.defaultProvider;
    kind.value = "quick";
    site.value = props.initialSite ?? null;
    hostname.value = "";
    hostEdited.value = false;
    resetUrlChoice();
  },
  { immediate: true },
);

const taken = computed(
  () =>
    new Set(props.items.filter((i) => i.provider === provider.value && i.kind === kind.value).map((i) => i.site)),
);

const options = computed(() =>
  props.sites
    .filter((s) => !taken.value.has(s.name))
    .map((s) => ({ value: s.name, label: s.name, sublabel: `PHP ${s.php}${s.secure ? " · https" : ""}` })),
);

const chosen = computed(() => props.sites.find((s) => s.name === site.value));

watch([kind, provider], () => {
  if (site.value && taken.value.has(site.value)) site.value = null;
});

watch(provider, (next) => {
  if (next !== "cloudflare") kind.value = "quick";
  resetUrlChoice();
});

watch([site, () => props.zone], ([name, zone]) => {
  if (!hostEdited.value) hostname.value = name ? suggestedHost(name, zone) : "";
});

/** The other providers the chosen site is already shared through. */
const sharedElsewhere = computed(() => {
  const name = site.value;
  if (!name) return [];
  const others = props.items.filter((i) => i.site === name && i.provider !== provider.value).map((i) => i.provider);
  return PROVIDER_ORDER.filter((p) => others.includes(p)).map(providerName);
});

function editHost(value: string): void {
  hostname.value = value;
  hostEdited.value = true;
}

function pick(next: ShareKind): void {
  if (next === "domain" && !props.domainReady) return;
  kind.value = next;
}

function pickUrl(next: "static" | "random"): void {
  if (next === "static" && !staticFree.value) return;
  urlChoice.value = next;
}

const canShare = computed(() => {
  if (!site.value || props.locked || !providerReady.value) return false;
  if (provider.value !== "cloudflare" || kind.value === "quick") return true;
  return props.domainReady && isShareHostname(hostname.value, props.zone);
});

const hostHint = computed(() => {
  if (!props.zone) return "Yerd couldn't read your Cloudflare domain yet. Sharing on it unlocks once it's known.";
  const host = hostname.value.trim();
  if (host && !isShareHostname(host, props.zone)) return `Use a hostname on ${props.zone}, like app.${props.zone}.`;
  return null;
});

function onOpen(open: boolean): void {
  if (!props.busy) emit("update:open", open);
}

function submit(): void {
  if (!canShare.value || !site.value) return;
  const domain = provider.value === "cloudflare" && kind.value === "domain";
  emit("share", {
    site: site.value,
    provider: provider.value,
    kind: kind.value,
    hostname: domain ? hostname.value.trim() : null,
    randomUrl: provider.value === "ngrok" && staticFree.value && urlChoice.value === "random",
  });
}

const LABEL = "mb-1.5 block text-xs font-medium";
const CARD_BTN =
  "flex min-h-24 min-w-0 flex-col gap-1.5 rounded-lg border px-3.5 py-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60";
const OPTION_BTN =
  "flex w-full items-start gap-3 rounded-lg border px-3.5 py-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60";
</script>

<template>
  <DialogFrame :open="open" title="Share a site" width="lg" @update:open="onOpen">
    <div class="space-y-4">
      <div>
        <span :class="LABEL">Site</span>
        <Combobox
          v-model="site"
          :options="options"
          placeholder="Choose a site…"
          search-placeholder="Search sites…"
          empty-text="No sites left to share this way."
          aria-label="Site"
          :disabled="busy"
        />
      </div>

      <div>
        <span :class="LABEL">Provider</span>
        <div class="grid grid-cols-3 gap-2" role="radiogroup" aria-label="Provider">
          <button
            v-for="p in PROVIDER_ORDER"
            :key="p"
            type="button"
            role="radio"
            :aria-checked="provider === p"
            :data-provider="p"
            :disabled="busy"
            :class="cn(CARD_BTN, 'min-h-0', provider === p ? 'border-brand bg-brand/[0.14]' : 'bg-background hover:bg-accent/60')"
            @click="provider = p"
          >
            <ProviderTile :provider="p" />
            <span class="text-[13px] font-semibold">{{ providerName(p) }}</span>
            <span class="flex items-center gap-1.5 text-xs text-muted-foreground">
              <span
                :class="cn('size-1.5 shrink-0 rounded-full', readinessDot(statusOf(p)?.readiness ?? 'not_installed'))"
                aria-hidden="true"
              />
              {{ readinessLabel(statusOf(p)?.readiness ?? "not_installed") }}
            </span>
          </button>
        </div>
      </div>

      <div
        v-if="!providerReady"
        class="flex items-center gap-3 rounded-lg border border-warning/40 bg-warning/10 px-3.5 py-3"
        data-testid="provider-not-ready"
      >
        <div class="min-w-0 flex-1">
          <p class="text-[13px] font-semibold">{{ providerName(provider) }} needs setting up first</p>
          <p class="mt-0.5 text-xs text-muted-foreground">Install it and finish its setup in Providers.</p>
        </div>
        <Button variant="outline" size="sm" :disabled="busy" data-testid="provider-setup" @click="emit('setup', provider)">
          <KeyRound /> Set up {{ providerName(provider) }}
        </Button>
      </div>

      <div v-else-if="provider === 'cloudflare'">
        <span :class="LABEL">URL</span>
        <div class="grid grid-cols-2 gap-2" role="radiogroup" aria-label="URL">
          <button
            type="button"
            role="radio"
            :aria-checked="kind === 'quick'"
            data-kind="quick"
            :disabled="busy"
            :class="cn(CARD_BTN, kind === 'quick' ? 'border-brand bg-brand/[0.14]' : 'bg-background hover:bg-accent/60')"
            @click="pick('quick')"
          >
            <span class="flex items-center gap-2 text-[13px] font-semibold">
              <Link class="size-3.5 text-muted-foreground" aria-hidden="true" /> Quick link
            </span>
            <span class="text-xs text-muted-foreground">Temporary trycloudflare.com URL. No account needed.</span>
          </button>
          <button
            type="button"
            role="radio"
            :aria-checked="kind === 'domain'"
            data-kind="domain"
            :disabled="busy || !domainReady"
            :class="cn(CARD_BTN, kind === 'domain' ? 'border-brand bg-brand/[0.14]' : 'bg-background hover:bg-accent/60 disabled:hover:bg-background')"
            @click="pick('domain')"
          >
            <span class="flex items-center gap-2 text-[13px] font-semibold">
              <Globe class="size-3.5 text-muted-foreground" aria-hidden="true" /> Your domain
            </span>
            <span class="text-xs text-muted-foreground" data-testid="domain-detail">
              <template v-if="domainReady">
                Stable hostname on {{ zone ?? "your domain" }} through your tunnel.
              </template>
              <template v-else>Needs a Cloudflare account and a tunnel. Set them up in Setup.</template>
            </span>
          </button>
        </div>
      </div>

      <div v-else-if="provider === 'ngrok' && ngrokDomain">
        <span :class="LABEL">URL</span>
        <div class="flex flex-col gap-2" role="radiogroup" aria-label="URL">
          <button
            type="button"
            role="radio"
            :aria-checked="urlChoice === 'static'"
            data-url="static"
            :disabled="busy || !staticFree"
            :class="cn(OPTION_BTN, urlChoice === 'static' ? 'border-brand bg-brand/[0.14]' : 'bg-background hover:bg-accent/60 disabled:hover:bg-background')"
            @click="pickUrl('static')"
          >
            <span class="min-w-0">
              <span class="block text-[13px] font-semibold">Static domain</span>
              <span class="block truncate font-mono text-xs text-muted-foreground">{{ ngrokDomain }}</span>
              <span v-if="domainHolder" class="block text-xs text-warning" data-testid="static-in-use">
                In use by {{ domainHolder }}'s share
              </span>
            </span>
          </button>
          <button
            type="button"
            role="radio"
            :aria-checked="urlChoice === 'random'"
            data-url="random"
            :disabled="busy"
            :class="cn(OPTION_BTN, urlChoice === 'random' ? 'border-brand bg-brand/[0.14]' : 'bg-background hover:bg-accent/60')"
            @click="pickUrl('random')"
          >
            <span class="min-w-0">
              <span class="block text-[13px] font-semibold">Random URL</span>
              <span class="block text-xs text-muted-foreground">ngrok picks the address</span>
            </span>
          </button>
        </div>
      </div>

      <div v-else>
        <span :class="LABEL">URL</span>
        <p class="text-xs text-muted-foreground" data-testid="url-line">
          <template v-if="provider === 'ngrok'">ngrok picks the address. Save a static domain in Providers to keep one.</template>
          <template v-else>zrok gives the share a new public URL each time it starts.</template>
        </p>
      </div>

      <div v-if="providerReady && provider === 'cloudflare' && kind === 'domain'">
        <label :class="LABEL" for="share-hostname">Public hostname</label>
        <Input
          id="share-hostname"
          class="font-mono"
          :model-value="hostname"
          :placeholder="hostPlaceholder(zone)"
          :disabled="busy"
          @update:model-value="editHost"
        />
        <p v-if="hostHint" class="mt-1.5 text-xs text-warning" data-testid="host-hint">{{ hostHint }}</p>
      </div>

      <InfoBanner v-if="site && sharedElsewhere.length" data-testid="shared-elsewhere">
        {{ site }} is already shared through {{ joinWithAnd(sharedElsewhere) }}. This adds a second public URL; both
        stay live.
      </InfoBanner>

      <p v-if="chosen" class="flex items-center gap-2 text-xs text-muted-foreground" data-testid="share-publishes">
        <Lock class="size-3.5 shrink-0" aria-hidden="true" />
        Publishes {{ chosen.name }}.{{ tld }} over {{ scheme(chosen) }} through {{ providerName(provider) }}.
      </p>
    </div>

    <template #footer>
      <Button variant="outline" :disabled="busy" @click="onOpen(false)">Cancel</Button>
      <Button :disabled="!canShare" data-testid="share-submit" @click="submit">
        <Spinner v-if="busy" class="size-4" />
        <Globe v-else />
        Share
      </Button>
    </template>
  </DialogFrame>
</template>
