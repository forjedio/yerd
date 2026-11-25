<script setup lang="ts">
import { computed, ref } from "vue";
import { Download, RefreshCw } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { CloudflaredStatus, NamedTunnelMeta, TunnelInfo } from "@/ipc/types";
import { readinessLabel } from "@/lib/providers";
import { canStartNamed, tunnelChip } from "@/lib/share";
import { CHIP_TONE, cn } from "@/lib/utils";

import PaneHeader from "./PaneHeader.vue";
import { BOX, CHIP, FIELD, FOOTER, HELP, LABEL, LINK, PANE, SECTION } from "./paneStyles";

/**
 * The Cloudflare pane of the Tunnel providers sheet: which cloudflared binary
 * is in use (installing and updating it happen in Tooling, linked from here),
 * the Cloudflare account, and the domain tunnel (create, start a stopped or
 * failed one, remove). Actions that open another dialog or page are handed up
 * to the view, which closes the sheet first so dialogs never stack. The
 * default slot fills the right of the bottom row (the sheet's Done).
 */
const props = defineProps<{
  cloudflared: CloudflaredStatus | null;
  zone: string | null;
  tunnel: NamedTunnelMeta | null;
  /** The named tunnel instance, in any state. */
  entry: TunnelInfo | null;
  /** How many sites are mapped onto the tunnel. */
  exposedCount: number;
  /** The view's in-flight action key, e.g. `create` or `named`. */
  busy: string | null;
  connected: boolean;
}>();

const emit = defineEmits<{
  tooling: [];
  login: [];
  create: [name: string];
  start: [];
  remove: [];
  "open-url": [url: string];
}>();

const newName = ref("");

const installed = computed(() => props.cloudflared?.installed ?? false);
const loggedIn = computed(() => props.cloudflared?.logged_in ?? false);
const system = computed(() => props.cloudflared?.source === "system");
const update = computed(() => (system.value ? null : (props.cloudflared?.update_available ?? null)));
const locked = computed(() => props.busy !== null || !props.connected);
const canStart = computed(() => canStartNamed(props.entry, props.exposedCount));
const showChip = computed(() => props.entry !== null || props.exposedCount > 0);

const chipClass = computed(() => {
  const state = props.entry?.state;
  if (state === "running") return CHIP_TONE.success;
  if (state === "failed") return CHIP_TONE.danger;
  if (state === "connecting" || state === "reconnecting") return CHIP_TONE.warning;
  return CHIP_TONE.neutral;
});

const readiness = computed(() => readinessLabel(installed.value ? "ready" : "not_installed"));

</script>

<template>
  <div :class="PANE">
    <PaneHeader
      provider="cloudflare"
      :readiness="readiness"
      :ready="installed"
      url="https://dash.cloudflare.com"
      @open-url="(url: string) => emit('open-url', url)"
    />

    <section :class="SECTION" data-testid="setup-cloudflared">
      <p :class="LABEL">Binary</p>
      <div v-if="installed" :class="BOX">
        <div class="min-w-0 flex-1">
          <p class="flex items-center gap-2 text-[13px] font-semibold">
            {{ cloudflared?.version ? `cloudflared ${cloudflared.version}` : "cloudflared" }}
            <span :class="cn(CHIP, CHIP_TONE.success)">Installed</span>
          </p>
          <p class="mt-[3px] font-mono text-[11.5px] text-muted-foreground" data-testid="cloudflared-source">
            <template v-if="system">Found on PATH · used for every tunnel</template>
            <template v-else-if="update">Yerd's copy · update to {{ update }} available in Tooling</template>
            <template v-else>Yerd's copy · used for every tunnel</template>
          </p>
        </div>
        <Button v-if="update" size="sm" variant="outline" :disabled="!connected" data-action="tooling" @click="emit('tooling')">
          <Download /> Update
        </Button>
      </div>
      <div v-else class="flex items-center gap-3 rounded-[9px] border border-warning/50 bg-warning/10 p-3.5">
        <div class="min-w-0 flex-1">
          <p class="text-[13px] font-semibold">cloudflared isn't installed</p>
          <p :class="cn(HELP, 'mt-1')">Needed for every Cloudflare share. Install it from Tooling.</p>
        </div>
        <Button size="sm" :disabled="!connected" data-action="tooling" @click="emit('tooling')">Open Tooling</Button>
      </div>
    </section>

    <section :class="SECTION" data-testid="setup-account">
      <p :class="LABEL">Account</p>
      <div :class="BOX">
        <div class="min-w-0 flex-1">
          <template v-if="loggedIn">
            <p class="flex items-center gap-2 text-[13px] font-semibold">
              Connected
              <span :class="cn(CHIP, CHIP_TONE.success)">Connected</span>
            </p>
            <p class="mt-[3px] font-mono text-[11.5px] text-muted-foreground">
              {{ zone ? `Domain ${zone}` : "Domain not resolved yet" }}
            </p>
          </template>
          <template v-else>
            <p class="text-[13px] font-semibold">Not connected</p>
            <p class="mt-[3px] text-[11.5px] text-muted-foreground">
              {{ tunnel ? "Reconnect to manage your tunnel." : "Needed for shares on your own domain." }}
            </p>
          </template>
        </div>
        <Button v-if="!loggedIn" size="sm" :disabled="locked || !installed" @click="emit('login')">
          <Spinner v-if="busy === 'login'" class="size-4" />
          Connect
        </Button>
      </div>
    </section>

    <section :class="SECTION" data-testid="setup-tunnel">
      <p :class="LABEL">Tunnel for your domain</p>
      <div v-if="tunnel" :class="BOX">
        <div class="min-w-0 flex-1">
          <p class="flex items-center gap-2 text-[13px] font-semibold">
            <span class="truncate">{{ tunnel.name }}</span>
            <span v-if="showChip" :class="cn(CHIP, chipClass)" data-testid="tunnel-chip">
              {{ tunnelChip(entry, exposedCount) }}
            </span>
          </p>
          <p class="mt-[3px] truncate font-mono text-[11.5px] text-muted-foreground">{{ tunnel.uuid }}</p>
        </div>
        <Button
          v-if="canStart"
          size="sm"
          variant="outline"
          :disabled="locked || !loggedIn"
          :title="loggedIn ? undefined : 'Connect your Cloudflare account first'"
          data-action="start"
          @click="emit('start')"
        >
          <Spinner v-if="busy === 'named'" class="size-4" />
          <RefreshCw v-else-if="entry" />
          {{ entry ? "Restart" : "Start" }}
        </Button>
      </div>
      <div v-else-if="loggedIn" class="flex items-center gap-2">
        <Input
          v-model="newName"
          placeholder="yerd-mac"
          aria-label="Tunnel name"
          :class="cn(FIELD, 'flex-1')"
          :disabled="locked"
        />
        <Button size="sm" :disabled="locked || !newName.trim()" @click="emit('create', newName.trim())">
          <Spinner v-if="busy === 'create'" class="size-4" />
          Create
        </Button>
      </div>
      <p v-else :class="HELP">Connect your Cloudflare account first.</p>
      <p v-if="tunnel && entry?.last_error" class="text-[11.5px] text-destructive" data-testid="tunnel-error">
        {{ entry.last_error }}
      </p>
      <p :class="HELP">Quick links need no account. Your-domain shares route through this tunnel.</p>
    </section>

    <div :class="FOOTER">
      <button
        v-if="tunnel"
        type="button"
        :class="cn(LINK, 'text-destructive')"
        :disabled="locked || !loggedIn"
        data-action="remove"
        @click="emit('remove')"
      >
        <Spinner v-if="busy === 'delete-tunnel'" class="mr-1 inline size-3" />
        Remove tunnel
      </button>
      <span v-else />
      <slot />
    </div>
  </div>
</template>
