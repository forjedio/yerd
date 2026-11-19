<script setup lang="ts">
import { ArrowRight, CornerDownRight, Globe, Lock, LockOpen, Plus } from "lucide-vue-next";
import { RadioGroupItem, RadioGroupRoot } from "reka-ui";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import SuffixInput from "@/components/ui/SuffixInput.vue";
import Switch from "@/components/ui/Switch.vue";
import type { SiteEntry } from "@/ipc/types";
import {
  addSummary,
  hasScheme,
  isValidPrefix,
  isValidProxyName,
  normalizeUpstream,
  proxyNameFromInput,
} from "@/lib/proxies";
import { cn } from "@/lib/utils";

/** Which kind of proxy the dialog adds. */
export type NewProxyMode = "domain" | "path";

/** A whole-domain proxy to add. */
export interface AddProxyRequest {
  name: string;
  url: string;
  secure: boolean;
}

/** A path rule to add on a site. */
export interface AddPathRequest {
  site: string;
  prefix: string;
  url: string;
}

/**
 * New proxy: one dialog with two modes. "A whole domain" gives a `.test` name to
 * an upstream (optionally over HTTPS); "A path on a site" hands one path prefix
 * of a site to an upstream while PHP serves the rest. Fields reset on each open.
 * Submitting closes the dialog, then emits the normalised request.
 */
const props = defineProps<{
  open: boolean;
  initialMode?: NewProxyMode;
  tld: string;
  sites: SiteEntry[];
  /** The local CA's system trust (`null` when unknown). */
  trusted: boolean | null;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  addProxy: [AddProxyRequest];
  addPath: [AddPathRequest];
}>();

const mode = ref<NewProxyMode>("domain");
const name = ref("");
const url = ref("");
const secure = ref(false);
const site = ref("");
const prefix = ref("");

const siteOptions = computed(() =>
  [...props.sites]
    .sort((a, b) => a.name.localeCompare(b.name))
    .map((s) => ({ value: s.name, label: `${s.name}.${props.tld}` })),
);
const hasSites = computed(() => siteOptions.value.length > 0);
const selectedSite = computed(() => props.sites.find((s) => s.name === site.value) ?? null);
const siteHostLabel = computed(() => `${site.value || "site"}.${props.tld}`);

function reset(): void {
  mode.value = props.initialMode === "path" && hasSites.value ? "path" : "domain";
  name.value = "";
  url.value = "";
  secure.value = false;
  site.value = siteOptions.value[0]?.value ?? "";
  prefix.value = "";
}

watch(
  () => props.open,
  (open) => {
    if (open) reset();
  },
  { immediate: true },
);

const proxyName = computed(() => proxyNameFromInput(name.value, props.tld));
const nameInvalid = computed(() => name.value.trim() !== "" && !isValidProxyName(proxyName.value));
const showScheme = computed(() => !hasScheme(url.value));

const valid = computed(() => {
  if (url.value.trim() === "") return false;
  if (mode.value === "domain") return proxyName.value !== "" && isValidProxyName(proxyName.value);
  return site.value !== "" && isValidPrefix(prefix.value);
});

const summary = computed(() =>
  mode.value === "domain"
    ? addSummary({ mode: "domain", name: name.value, url: url.value, tld: props.tld })
    : addSummary({ mode: "path", site: site.value, prefix: prefix.value, url: url.value, tld: props.tld }),
);

const httpsSubtitle = computed(() =>
  props.trusted === true
    ? "Certificate from Yerd local CA — trusted"
    : "Certificate from Yerd local CA — not trusted, browsers will warn",
);

function onMode(value: unknown): void {
  if (value === "domain" || value === "path") mode.value = value;
}

watch(siteOptions, (options) => {
  if (!options.some((o) => o.value === site.value)) site.value = options[0]?.value ?? "";
});

/** Return in a text field submits; elsewhere (the switch, the select) it keeps
 *  its usual meaning. */
function onEnter(event: KeyboardEvent): void {
  if (!(event.target instanceof HTMLInputElement)) return;
  event.preventDefault();
  submit();
}

function submit(): void {
  if (!valid.value) return;
  const upstream = normalizeUpstream(url.value);
  emit("update:open", false);
  if (mode.value === "domain") {
    emit("addProxy", { name: proxyName.value, url: upstream, secure: secure.value });
  } else {
    emit("addPath", { site: site.value, prefix: prefix.value.trim(), url: upstream });
  }
}

const MODES = [
  { value: "domain", label: "A whole domain", icon: Globe },
  { value: "path", label: "A path on a site", icon: CornerDownRight },
] as const;
</script>

<template>
  <DialogFrame :open="open" title="New proxy" @update:open="emit('update:open', $event)">
    <RadioGroupRoot
      :model-value="mode"
      orientation="horizontal"
      aria-label="Proxy kind"
      class="grid grid-cols-2 gap-1 rounded-lg border bg-background p-1"
      @update:model-value="onMode"
    >
      <RadioGroupItem
        v-for="m in MODES"
        :key="m.value"
        :value="m.value"
        :disabled="m.value === 'path' && !hasSites"
        :title="m.value === 'path' && !hasSites ? 'Add a site first' : undefined"
        :data-mode="m.value"
        :class="
          cn(
            'flex h-8 items-center justify-center gap-2 rounded-md text-[13px] transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed disabled:opacity-50',
            mode === m.value
              ? 'bg-card font-semibold text-foreground shadow-sm'
              : 'text-muted-foreground hover:text-foreground',
          )
        "
      >
        <component :is="m.icon" class="size-3.5" aria-hidden="true" />
        {{ m.label }}
      </RadioGroupItem>
    </RadioGroupRoot>

    <div @keydown.enter="onEnter">
      <template v-if="mode === 'domain'">
        <label class="mt-5 block text-[13px] font-semibold" for="proxy-name">Domain</label>
        <SuffixInput
          id="proxy-name"
          v-model="name"
          class="mt-2"
          :suffix="`.${tld}`"
          placeholder="grafana"
          aria-label="Domain"
          :invalid="nameInvalid"
        />
        <p class="mt-1.5 text-xs text-faint">
          Dots are fine — api.account serves api.account.{{ tld }}
        </p>
      </template>

      <template v-else>
        <label class="mt-5 block text-[13px] font-semibold" for="proxy-prefix">Path</label>
        <div class="mt-2 flex gap-2">
          <Select
            id="proxy-site"
            v-model="site"
            class="h-10 w-[250px] shrink-0 font-mono text-[13px]"
            :options="siteOptions"
            aria-label="Site"
          />
          <Input
            id="proxy-prefix"
            v-model="prefix"
            class="h-10 font-mono"
            placeholder="/live"
            aria-label="Path"
          />
        </div>
        <p class="mt-1.5 text-xs text-faint">
          PHP keeps serving the rest of {{ siteHostLabel }}. HTTPS follows the site.
        </p>
      </template>

      <label class="mt-5 block text-[13px] font-semibold" for="proxy-url">Forward to</label>
      <div
        class="mt-2 flex h-10 cursor-text items-center rounded-lg border bg-background px-3 font-mono text-sm transition-colors focus-within:border-brand focus-within:ring-1 focus-within:ring-brand"
      >
        <span v-if="showScheme" class="mr-1 shrink-0 text-faint" data-testid="scheme-hint">http://</span>
        <input
          id="proxy-url"
          v-model="url"
          class="min-w-0 flex-1 bg-transparent outline-none placeholder:text-faint"
          :placeholder="mode === 'domain' ? 'localhost:3000' : '127.0.0.1:8081'"
          spellcheck="false"
          autocomplete="off"
        />
      </div>

      <div
        v-if="mode === 'domain'"
        class="mt-5 flex items-center justify-between gap-4 rounded-lg border bg-background px-3.5 py-2.5"
      >
        <div class="min-w-0">
          <p class="text-[13px] font-semibold">HTTPS</p>
          <p class="truncate text-[11px] text-faint" data-testid="dialog-https-subtitle">
            {{ httpsSubtitle }}
          </p>
        </div>
        <Switch v-model="secure" aria-label="HTTPS" />
      </div>

      <div
        v-else
        class="mt-5 flex items-center gap-2.5 rounded-lg border bg-background px-3.5 py-3 text-[13px] text-muted-foreground"
        data-testid="wss-row"
      >
        <template v-if="selectedSite?.secure">
          <Lock class="size-3.5 shrink-0 text-success" aria-hidden="true" />
          <span class="min-w-0">
            {{ siteHostLabel }} is on HTTPS, so wss://{{ siteHostLabel }}{{ prefix.trim() || "/path" }} works too
          </span>
        </template>
        <template v-else>
          <LockOpen class="size-3.5 shrink-0 text-faint" aria-hidden="true" />
          <span class="min-w-0">
            {{ siteHostLabel }} is on HTTP. Turn on HTTPS for the site to use wss://
          </span>
        </template>
      </div>
    </div>

    <template #footer-start>
      <span class="flex min-w-0 items-center gap-1.5 font-mono" data-testid="add-summary">
        <span class="truncate">{{ summary.from }}</span>
        <ArrowRight class="size-3 shrink-0" aria-hidden="true" />
        <span class="truncate">{{ summary.to }}</span>
      </span>
    </template>
    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Cancel</Button>
      <Button :disabled="!valid" @click="submit">
        <Plus /> {{ mode === "domain" ? "Add proxy" : "Add path" }}
      </Button>
    </template>
  </DialogFrame>
</template>
