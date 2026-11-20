<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Download } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { SECTION_LABEL } from "@/components/ui/styles";
import type { CloudflaredStatus, NamedTunnelMeta, TunnelInfo } from "@/ipc/types";
import { canStartNamed, tunnelChip } from "@/lib/share";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * Share setup: the cloudflared binary (install, update, or switch from a PATH
 * copy to Yerd's), the Cloudflare account, and the domain tunnel (create,
 * start a stopped or failed one, remove). Actions that open another dialog are
 * handed to the view, which closes this one first so dialogs never stack.
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
  /** The view's in-flight action key, e.g. `create` or `named`. */
  busy: string | null;
  /** A cloudflared install or update is running. */
  installing: boolean;
  connected: boolean;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  install: [];
  login: [];
  create: [name: string];
  start: [];
  remove: [];
}>();

const newName = ref("");

watch(
  () => props.open,
  (open) => {
    if (open) newName.value = "";
  },
);

const installed = computed(() => props.cloudflared?.installed ?? false);
const loggedIn = computed(() => props.cloudflared?.logged_in ?? false);
const system = computed(() => props.cloudflared?.source === "system");
const update = computed(() => (system.value ? null : (props.cloudflared?.update_available ?? null)));
const locked = computed(() => props.busy !== null || props.installing || !props.connected);
const canStart = computed(() => canStartNamed(props.entry, props.exposedCount));
const showChip = computed(() => props.entry !== null || props.exposedCount > 0);

const chipClass = computed(() => {
  const state = props.entry?.state;
  if (state === "running") return CHIP_TONE.success;
  if (state === "failed") return CHIP_TONE.danger;
  if (state === "connecting" || state === "reconnecting") return CHIP_TONE.warning;
  return CHIP_TONE.neutral;
});

const SECTION = "py-4 first:pt-0";
const CHIP = "inline-flex shrink-0 items-center gap-1.5 rounded px-2 py-0.5 text-[11px] font-semibold";
const LINK =
  "shrink-0 rounded text-xs font-medium hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";
</script>

<template>
  <DialogFrame :open="open" title="Share setup" width="lg" @update:open="(v: boolean) => emit('update:open', v)">
    <div class="divide-y">
      <section :class="SECTION" data-testid="setup-cloudflared">
        <p :class="SECTION_LABEL">cloudflared</p>
        <div class="mt-2.5 flex items-center gap-3">
          <div class="min-w-0 flex-1">
            <template v-if="installed">
              <p class="text-[13px] font-semibold">
                {{ cloudflared?.version ? `cloudflared ${cloudflared.version}` : "cloudflared" }}
              </p>
              <p class="text-xs text-muted-foreground">
                {{ system ? "Found on PATH" : "Yerd's copy" }} · used for every tunnel
              </p>
            </template>
            <template v-else>
              <p class="text-[13px] font-semibold">Not installed</p>
              <p class="text-xs text-muted-foreground">Needed for every share.</p>
            </template>
          </div>
          <span v-if="installed" :class="cn(CHIP, 'bg-success/15 uppercase text-success')">Installed</span>
          <Button v-if="!installed" size="sm" :disabled="locked" @click="emit('install')">
            <Spinner v-if="installing" class="size-4" />
            <Download v-else />
            Install
          </Button>
          <Button v-else-if="system" size="sm" variant="outline" :disabled="locked" @click="emit('install')">
            <Spinner v-if="installing" class="size-4" />
            Use Yerd's copy
          </Button>
          <Button v-else-if="update" size="sm" variant="outline" :disabled="locked" @click="emit('install')">
            <Spinner v-if="installing" class="size-4" />
            <Download v-else />
            Update to {{ update }}
          </Button>
        </div>
      </section>

      <section :class="SECTION" data-testid="setup-account">
        <p :class="SECTION_LABEL">Cloudflare account</p>
        <div class="mt-2.5 flex items-center gap-3">
          <div class="min-w-0 flex-1">
            <template v-if="loggedIn">
              <p class="text-[13px] font-semibold">Connected</p>
              <p class="text-xs text-muted-foreground">{{ zone ? `Domain ${zone}` : "Domain not resolved yet" }}</p>
            </template>
            <template v-else>
              <p class="text-[13px] font-semibold">Not connected</p>
              <p class="text-xs text-muted-foreground">
                {{ tunnel ? "Reconnect to manage your tunnel." : "Needed for shares on your own domain." }}
              </p>
            </template>
          </div>
          <span v-if="loggedIn" :class="cn(CHIP, CHIP_TONE.success)">
            <span class="size-1.5 rounded-full bg-success" aria-hidden="true" /> Connected
          </span>
          <Button v-else size="sm" :disabled="locked || !installed" @click="emit('login')">
            <Spinner v-if="busy === 'login'" class="size-4" />
            Connect
          </Button>
        </div>
      </section>

      <section :class="SECTION" data-testid="setup-tunnel">
        <p :class="SECTION_LABEL">Tunnel for your domain</p>
        <template v-if="tunnel">
          <div class="mt-2.5 flex items-center gap-3">
            <div class="min-w-0 flex-1">
              <p class="truncate text-[13px] font-semibold">{{ tunnel.name }}</p>
              <p class="truncate font-mono text-[11px] text-faint">{{ tunnel.uuid }}</p>
            </div>
            <span v-if="showChip" :class="cn(CHIP, chipClass)" data-testid="tunnel-chip">
              <span class="size-1.5 rounded-full bg-current" aria-hidden="true" />
              {{ tunnelChip(entry, exposedCount) }}
            </span>
            <button
              v-if="canStart"
              type="button"
              :class="cn(LINK, 'text-brand')"
              :disabled="locked || !loggedIn"
              :title="loggedIn ? undefined : 'Connect your Cloudflare account first'"
              data-action="start"
              @click="emit('start')"
            >
              <Spinner v-if="busy === 'named'" class="mr-1 inline size-3" />
              {{ entry ? "Restart" : "Start" }}
            </button>
          </div>
          <p v-if="entry?.last_error" class="mt-2 text-xs text-destructive" data-testid="tunnel-error">
            {{ entry.last_error }}
          </p>
          <button
            type="button"
            :class="cn(LINK, 'mt-3 text-destructive')"
            :disabled="locked || !loggedIn"
            data-action="remove"
            @click="emit('remove')"
          >
            <Spinner v-if="busy === 'delete-tunnel'" class="mr-1 inline size-3" />
            Remove tunnel
          </button>
        </template>
        <div v-else-if="loggedIn" class="mt-2.5 flex items-center gap-2">
          <Input v-model="newName" placeholder="yerd-mac" aria-label="Tunnel name" class="flex-1" :disabled="locked" />
          <Button size="sm" :disabled="locked || !newName.trim()" @click="emit('create', newName.trim())">
            <Spinner v-if="busy === 'create'" class="size-4" />
            Create
          </Button>
        </div>
        <p v-else class="mt-2.5 text-xs text-muted-foreground">Connect your Cloudflare account first.</p>
      </section>
    </div>

    <template #footer>
      <Button @click="emit('update:open', false)">Done</Button>
    </template>
  </DialogFrame>
</template>
