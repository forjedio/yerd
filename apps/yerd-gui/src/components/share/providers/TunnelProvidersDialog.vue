<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ChevronDown, X } from "lucide-vue-next";
import { DialogClose, DialogContent, DialogDescription, DialogOverlay, DialogPortal, DialogRoot, DialogTitle } from "reka-ui";

import Button from "@/components/ui/Button.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { keepOpenForToaster } from "@/components/ui/toaster";
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

let loadSeq = 0;

/** Fetch the providers; only the most recent request's reply is applied. */
async function load(): Promise<void> {
  const seq = ++loadSeq;
  loading.value = true;
  try {
    const reply = await listTunnelProviders();
    if (seq !== loadSeq) return;
    providers.value = reply.providers;
    defaultProvider.value = reply.default_provider;
  } catch (e) {
    if (seq !== loadSeq) return;
    toast.error("Couldn't load tunnel providers", (e as IpcError).message);
  } finally {
    if (seq === loadSeq) loading.value = false;
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

/**
 * Run one provider action under `key`, toast the outcome, then refetch. The
 * action stays busy until the refetch settles so the panes never act on stale
 * state.
 */
async function run(key: string, action: () => Promise<void>, done: string, failed: string): Promise<void> {
  actionBusy.value = key;
  try {
    try {
      await action();
      toast.success(done);
    } catch (e) {
      toast.error(failed, (e as IpcError).message);
    }
    await load();
  } finally {
    actionBusy.value = null;
  }
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
  "flex h-[50px] w-full items-center gap-2.5 rounded-[9px] border border-transparent px-2.5 text-left transition-colors hover:bg-muted focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
</script>

<template>
  <DialogRoot :open="open" @update:open="(v: boolean) => emit('update:open', v)">
    <DialogPortal>
      <DialogOverlay class="fixed inset-0 z-50 rounded-[10px] bg-black/50 animate-fade-in" />
      <DialogContent
        aria-modal="true"
        class="fixed left-1/2 top-1/2 z-50 flex max-h-[calc(100%-2rem)] w-[calc(100%-2rem)] max-w-[720px] -translate-x-1/2 -translate-y-1/2 flex-col overflow-hidden rounded-[14px] border bg-card text-card-foreground shadow-lg animate-fade-in focus:outline-none"
        @interact-outside="keepOpenForToaster"
      >
        <div class="flex shrink-0 items-start justify-between gap-3 border-b px-[22px] pb-3.5 pt-[18px]">
          <div class="flex flex-col gap-1">
            <DialogTitle class="text-base font-semibold">Tunnel providers</DialogTitle>
            <DialogDescription class="text-xs text-muted-foreground">
              Share through any provider that's ready. Each keeps its own login and its own URLs.
            </DialogDescription>
          </div>
          <DialogClose
            aria-label="Close"
            class="-mr-1 rounded-md p-1 text-faint transition-colors hover:bg-muted hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          >
            <X class="size-4" />
          </DialogClose>
        </div>

        <div class="flex h-[520px] min-h-0">
          <div class="flex w-[200px] shrink-0 flex-col justify-between border-r bg-sidebar px-3 py-3.5">
            <div class="flex flex-col gap-1">
              <button
                v-for="item in items"
                :key="item.provider"
                type="button"
                :class="cn(ITEM, selected === item.provider && 'border-brand/40 bg-selected hover:bg-selected')"
                :aria-pressed="selected === item.provider"
                :data-testid="`provider-${item.provider}`"
                @click="selected = item.provider"
              >
                <ProviderTile :provider="item.provider" />
                <span class="flex min-w-0 flex-col gap-0.5">
                  <span class="text-[13px] font-semibold">{{ providerName(item.provider) }}</span>
                  <span v-if="item.status" class="flex items-center gap-[5px] text-[11px] text-muted-foreground">
                    <span :class="cn('size-[7px] rounded-full', readinessDot(item.status.readiness))" aria-hidden="true" />
                    {{ readinessLabel(item.status.readiness) }}
                  </span>
                </span>
              </button>
            </div>
            <div class="flex flex-col gap-1.5">
              <p class="text-[11px] font-medium text-muted-foreground">Default for new shares</p>
              <div class="relative">
                <Select
                  :model-value="defaultProvider"
                  :options="DEFAULT_OPTIONS"
                  aria-label="Default for new shares"
                  class="h-8 w-full appearance-none rounded-lg pl-2.5 pr-8 text-[12.5px] font-medium shadow-none"
                  :disabled="!connected || actionBusy !== null"
                  @update:model-value="setDefault"
                />
                <ChevronDown
                  class="pointer-events-none absolute right-2.5 top-1/2 size-[13px] -translate-y-1/2 text-muted-foreground"
                  aria-hidden="true"
                />
              </div>
            </div>
          </div>

          <div class="min-w-0 flex-1 overflow-y-auto px-6 py-[18px]" data-testid="provider-pane">
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
            >
              <Button size="sm" @click="emit('update:open', false)">Done</Button>
            </CloudflarePane>
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
            >
              <Button size="sm" @click="emit('update:open', false)">Done</Button>
            </NgrokPane>
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
            >
              <Button size="sm" @click="emit('update:open', false)">Done</Button>
            </ZrokPane>
            <div v-else-if="loading" class="flex justify-center py-10"><Spinner class="size-5" /></div>
            <div v-else class="flex flex-col gap-4" data-testid="providers-unavailable">
              <p class="text-[13px] text-muted-foreground">
                Couldn't load {{ providerName(selected) }}'s status from the Yerd daemon.
              </p>
              <div class="flex items-center justify-between gap-3">
                <Button size="sm" variant="outline" :disabled="!connected" @click="load">Retry</Button>
                <Button size="sm" @click="emit('update:open', false)">Done</Button>
              </div>
            </div>
          </div>
        </div>
      </DialogContent>
    </DialogPortal>
  </DialogRoot>
</template>
