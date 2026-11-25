<script setup lang="ts">
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import {
  disableZrok,
  enableZrok,
  IpcError,
  listTunnelProviders,
  setDefaultTunnelProvider,
  setNgrokAuthtoken,
  setNgrokDomain,
} from "@/ipc/client";
import type { CloudflaredStatus, NamedTunnelMeta, TunnelInfo, TunnelProvider, TunnelProviderStatus } from "@/ipc/types";
import { DEFAULT_OPTIONS, PROVIDER_ORDER, providerName, readinessDot, readinessLabel } from "@/lib/providers";
import { cn } from "@/lib/utils";

import CloudflarePane from "./CloudflarePane.vue";
import NgrokPane from "./NgrokPane.vue";
import ProviderTile from "./ProviderTile.vue";
import ZrokPane from "./ZrokPane.vue";

/**
 * The Tunnel providers sheet: a provider list with readiness on the left, the
 * "Default for new shares" select under it, and the selected provider's pane
 * on the right. It loads the providers each time it opens and performs the
 * ngrok, zrok and default actions itself; Cloudflare's events are handed to
 * the view, which closes the sheet first so dialogs never stack.
 */
const props = defineProps<{
  open: boolean;
  cloudflared: CloudflaredStatus | null;
  zone: string | null;
  tunnel: NamedTunnelMeta | null;
  /** The named tunnel instance, in any state. */
  entry: TunnelInfo | null;
  /** How many sites are mapped onto the tunnel. */
  exposedCount: number;
  /** The view's in-flight action key, for the Cloudflare pane. */
  busy: string | null;
  connected: boolean;
  /** The provider selected when the sheet opens; Cloudflare when absent. */
  initial?: TunnelProvider | null;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  tooling: [];
  login: [];
  create: [name: string];
  start: [];
  remove: [];
  "open-url": [url: string];
}>();

const toast = useToast();
const selected = ref<TunnelProvider>("cloudflare");
const providers = ref<TunnelProviderStatus[]>([]);
const defaultProvider = ref<TunnelProvider>("cloudflare");
const loading = ref(false);
const actionBusy = ref<string | null>(null);

const statusOf = (p: TunnelProvider) => providers.value.find((s) => s.provider === p) ?? null;
const items = computed(() => PROVIDER_ORDER.map((provider) => ({ provider, status: statusOf(provider) })));
const ngrok = computed(() => statusOf("ngrok"));
const zrok = computed(() => statusOf("zrok"));

async function load(): Promise<void> {
  loading.value = true;
  try {
    const reply = await listTunnelProviders();
    providers.value = reply.providers;
    defaultProvider.value = reply.default_provider;
  } catch (e) {
    toast.error("Couldn't load tunnel providers", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

watch(
  () => props.open,
  (open) => {
    if (!open) return;
    selected.value = props.initial ?? "cloudflare";
    void load();
  },
  { immediate: true },
);

/** Run one provider action under `key`, toast the outcome, then refetch. */
async function run(key: string, action: () => Promise<void>, done: string, failed: string): Promise<void> {
  actionBusy.value = key;
  try {
    await action();
    toast.success(done);
  } catch (e) {
    toast.error(failed, (e as IpcError).message);
  } finally {
    actionBusy.value = null;
  }
  await load();
}

function saveToken(token: string): void {
  void run("ngrok-token", () => setNgrokAuthtoken(token), "Saved the ngrok authtoken", "Couldn't save the ngrok authtoken");
}

function removeToken(): void {
  void run("ngrok-token", () => setNgrokAuthtoken(null), "Removed the ngrok authtoken", "Couldn't remove the ngrok authtoken");
}

function saveDomain(domain: string | null): void {
  void run(
    "ngrok-domain",
    () => setNgrokDomain(domain),
    domain === null ? "Cleared the ngrok static domain" : "Saved the ngrok static domain",
    "Couldn't save the ngrok static domain",
  );
}

function enable(token: string): void {
  void run("zrok", () => enableZrok(token), "Enabled zrok on this machine", "Couldn't enable zrok");
}

function disable(): void {
  void run("zrok", () => disableZrok(), "Disabled zrok on this machine", "Couldn't disable zrok");
}

function setDefault(provider: TunnelProvider): void {
  defaultProvider.value = provider;
  void run(
    "default",
    () => setDefaultTunnelProvider(provider),
    `New shares use ${providerName(provider)}`,
    "Couldn't change the default provider",
  );
}

const ITEM =
  "flex w-full items-center gap-2.5 rounded-md px-2.5 py-2 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
</script>

<template>
  <DialogFrame :open="open" title="Tunnel providers" width="xl" @update:open="(v: boolean) => emit('update:open', v)">
    <p class="-mt-2 mb-4 text-xs text-muted-foreground">
      Share through any provider that's ready. Each keeps its own login and its own URLs.
    </p>
    <div class="grid grid-cols-[180px_1fr] gap-5">
      <div class="flex flex-col gap-1">
        <button
          v-for="item in items"
          :key="item.provider"
          type="button"
          :class="cn(ITEM, selected === item.provider && 'bg-brand/15 ring-1 ring-brand/40')"
          :aria-pressed="selected === item.provider"
          :data-testid="`provider-${item.provider}`"
          @click="selected = item.provider"
        >
          <ProviderTile :provider="item.provider" />
          <span class="min-w-0">
            <span class="block text-[13px] font-semibold">{{ providerName(item.provider) }}</span>
            <span v-if="item.status" class="flex items-center gap-1.5 text-[11px] text-muted-foreground">
              <span :class="cn('size-1.5 rounded-full', readinessDot(item.status.readiness))" aria-hidden="true" />
              {{ readinessLabel(item.status.readiness) }}
            </span>
          </span>
        </button>
        <div class="mt-auto pt-6">
          <p class="mb-1.5 text-[11px] text-muted-foreground">Default for new shares</p>
          <Select
            :model-value="defaultProvider"
            :options="DEFAULT_OPTIONS"
            aria-label="Default for new shares"
            class="w-full"
            :disabled="!connected || actionBusy !== null"
            @update:model-value="setDefault"
          />
        </div>
      </div>

      <div class="min-w-0" data-testid="provider-pane">
        <CloudflarePane
          v-if="selected === 'cloudflare'"
          :cloudflared="cloudflared"
          :zone="zone"
          :tunnel="tunnel"
          :entry="entry"
          :exposed-count="exposedCount"
          :busy="busy"
          :connected="connected"
          @tooling="emit('tooling')"
          @login="emit('login')"
          @create="(name: string) => emit('create', name)"
          @start="emit('start')"
          @remove="emit('remove')"
          @open-url="(url: string) => emit('open-url', url)"
        />
        <NgrokPane
          v-else-if="selected === 'ngrok' && ngrok"
          :status="ngrok"
          :busy="actionBusy"
          :connected="connected"
          @tooling="emit('tooling')"
          @recheck="load"
          @open-url="(url: string) => emit('open-url', url)"
          @save-token="saveToken"
          @remove-token="removeToken"
          @save-domain="saveDomain"
        />
        <ZrokPane
          v-else-if="selected === 'zrok' && zrok"
          :status="zrok"
          :busy="actionBusy"
          :connected="connected"
          @tooling="emit('tooling')"
          @recheck="load"
          @open-url="(url: string) => emit('open-url', url)"
          @enable="enable"
          @disable="disable"
        />
        <div v-else-if="loading" class="flex justify-center py-10"><Spinner class="size-5" /></div>
      </div>
    </div>

    <template #footer>
      <Button @click="emit('update:open', false)">Done</Button>
    </template>
  </DialogFrame>
</template>
