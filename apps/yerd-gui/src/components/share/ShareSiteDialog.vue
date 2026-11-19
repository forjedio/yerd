<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Globe, Link, Lock } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Combobox from "@/components/ui/Combobox.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { Site } from "@/ipc/types";
import { scheme, type ShareKind, suggestedHost } from "@/lib/share";
import { cn } from "@/lib/utils";

/** What the dialog asks the view to share. `hostname` is set for a domain share. */
export interface ShareRequest {
  site: string;
  kind: ShareKind;
  hostname: string | null;
}

/**
 * The one-step Share a site dialog: pick a site, then a Quick link (a temporary
 * trycloudflare.com URL) or Your domain (a hostname on the account's zone
 * through the domain tunnel). The hostname follows the site until edited.
 * While the share is in flight the dialog stays open and locked; the view
 * closes it on success.
 */
const props = defineProps<{
  open: boolean;
  sites: Site[];
  /** Sites that already have a quick link. */
  quickSites: string[];
  /** Sites already on the domain tunnel. */
  domainSites: string[];
  zone: string | null;
  /** Whether a Cloudflare account is connected and a tunnel exists. */
  domainReady: boolean;
  tld: string;
  /** A share from this dialog is in flight. */
  busy: boolean;
  /** Another action is in flight or the daemon is down. */
  locked: boolean;
}>();

const emit = defineEmits<{ "update:open": [boolean]; share: [request: ShareRequest] }>();

const kind = ref<ShareKind>("quick");
const site = ref<string | null>(null);
const hostname = ref("");
const hostEdited = ref(false);

watch(
  () => props.open,
  (open) => {
    if (!open) return;
    kind.value = "quick";
    site.value = null;
    hostname.value = "";
    hostEdited.value = false;
  },
  { immediate: true },
);

const taken = computed(() => new Set(kind.value === "quick" ? props.quickSites : props.domainSites));

const options = computed(() =>
  props.sites
    .filter((s) => !taken.value.has(s.name))
    .map((s) => ({ value: s.name, label: s.name, sublabel: `PHP ${s.php}${s.secure ? " · https" : ""}` })),
);

const chosen = computed(() => props.sites.find((s) => s.name === site.value));

watch(kind, () => {
  if (site.value && taken.value.has(site.value)) site.value = null;
});

watch([site, () => props.zone], ([name, zone]) => {
  if (!hostEdited.value) hostname.value = name ? suggestedHost(name, zone) : "";
});

function editHost(value: string): void {
  hostname.value = value;
  hostEdited.value = true;
}

function pick(next: ShareKind): void {
  if (next === "domain" && !props.domainReady) return;
  kind.value = next;
}

const canShare = computed(() => {
  if (!site.value || props.locked) return false;
  if (kind.value === "quick") return true;
  return props.domainReady && hostname.value.trim().length > 0;
});

function onOpen(open: boolean): void {
  if (!props.busy) emit("update:open", open);
}

function submit(): void {
  if (!canShare.value || !site.value) return;
  emit("share", {
    site: site.value,
    kind: kind.value,
    hostname: kind.value === "domain" ? hostname.value.trim() : null,
  });
}

const LABEL = "mb-1.5 block text-xs font-medium";
const CARD_BTN =
  "flex min-h-24 min-w-0 flex-col gap-1.5 rounded-lg border px-3.5 py-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-60";
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
        <span :class="LABEL">How</span>
        <div class="grid grid-cols-2 gap-2" role="radiogroup" aria-label="How">
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

      <div v-if="kind === 'domain'">
        <label :class="LABEL" for="share-hostname">Public hostname</label>
        <Input
          id="share-hostname"
          class="font-mono"
          :model-value="hostname"
          :placeholder="suggestedHost('site', zone)"
          :disabled="busy"
          @update:model-value="editHost"
        />
      </div>

      <p v-if="chosen" class="flex items-center gap-2 text-xs text-muted-foreground" data-testid="share-publishes">
        <Lock class="size-3.5 shrink-0" aria-hidden="true" />
        Publishes {{ chosen.name }}.{{ tld }} over {{ scheme(chosen) }}. Anyone with the address can reach it.
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
