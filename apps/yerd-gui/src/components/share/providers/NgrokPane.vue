<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ExternalLink } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { SECTION_LABEL } from "@/components/ui/styles";
import type { TunnelProviderStatus } from "@/ipc/types";
import { readinessLabel } from "@/lib/providers";
import { CHIP_TONE, cn } from "@/lib/utils";

import BinarySection from "./BinarySection.vue";
import ProviderTile from "./ProviderTile.vue";

/**
 * The ngrok pane of the Tunnel providers sheet: the binary, the authtoken
 * (only ever shown masked; saved, replaced or removed through the dialog) and
 * the optional static domain. Presentational: every action is emitted.
 */
const props = defineProps<{
  status: TunnelProviderStatus;
  /** The dialog's in-flight action key (`ngrok-token`, `ngrok-domain`, ...). */
  busy: string | null;
  connected: boolean;
}>();

const emit = defineEmits<{
  tooling: [];
  recheck: [];
  "open-url": [url: string];
  "save-token": [token: string];
  "remove-token": [];
  "save-domain": [domain: string | null];
}>();

const tokenInput = ref("");
const replacing = ref(false);
const domainInput = ref(props.status.domain ?? "");

watch(
  () => props.status.authtoken_hint,
  () => {
    tokenInput.value = "";
    replacing.value = false;
  },
);

watch(
  () => props.status.domain,
  (domain) => {
    domainInput.value = domain ?? "";
  },
);

const locked = computed(() => props.busy !== null || !props.connected);
const hasToken = computed(() => Boolean(props.status.authtoken_hint));
const editingToken = computed(() => !hasToken.value || replacing.value);
const domainChanged = computed(() => domainInput.value.trim() !== (props.status.domain ?? ""));
const ready = computed(() => props.status.readiness === "ready");

function saveToken(): void {
  const token = tokenInput.value.trim();
  if (token) emit("save-token", token);
}

function saveDomain(): void {
  emit("save-domain", domainInput.value.trim() ? domainInput.value : null);
}

function cancelReplace(): void {
  replacing.value = false;
  tokenInput.value = "";
}

const CHIP = "inline-flex shrink-0 items-center rounded px-2 py-0.5 text-[11px] font-semibold uppercase";
const LINK =
  "shrink-0 rounded text-xs font-medium hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";
const HELP = "mt-2 text-xs text-muted-foreground";
</script>

<template>
  <div>
    <div class="mb-4 flex items-center gap-2.5">
      <ProviderTile provider="ngrok" size="md" />
      <h3 class="text-[15px] font-semibold">ngrok</h3>
      <span :class="cn(CHIP, ready ? CHIP_TONE.success : CHIP_TONE.warning)" data-testid="pane-readiness">
        {{ readinessLabel(status.readiness) }}
      </span>
      <button
        type="button"
        :class="cn(LINK, 'ml-auto inline-flex items-center gap-1 text-brand')"
        @click="emit('open-url', 'https://dashboard.ngrok.com')"
      >
        dashboard.ngrok.com <ExternalLink class="size-3" />
      </button>
    </div>

    <div class="divide-y">
      <BinarySection
        :status="status"
        name="ngrok"
        bin="ngrok"
        :locked="locked"
        :connected="connected"
        @tooling="emit('tooling')"
        @recheck="emit('recheck')"
      />

      <section class="py-4" data-testid="ngrok-token">
        <p :class="SECTION_LABEL">Authtoken</p>
        <div v-if="!editingToken" class="mt-2.5 flex items-center gap-2">
          <p class="flex h-9 flex-1 items-center rounded-md border px-3 font-mono text-sm" data-testid="token-hint">
            {{ status.authtoken_hint }}
          </p>
          <Button size="sm" variant="outline" :disabled="locked" @click="replacing = true">Replace</Button>
        </div>
        <div v-else class="mt-2.5 flex items-center gap-2">
          <Input
            v-model="tokenInput"
            type="password"
            placeholder="Paste your ngrok authtoken"
            aria-label="ngrok authtoken"
            class="flex-1"
            :disabled="locked"
          />
          <Button v-if="replacing" size="sm" variant="ghost" :disabled="locked" @click="cancelReplace">Cancel</Button>
          <Button size="sm" :disabled="locked || !tokenInput.trim()" @click="saveToken">
            <Spinner v-if="busy === 'ngrok-token'" class="size-4" />
            Save
          </Button>
        </div>
        <p :class="HELP">
          Saved in Yerd's config and handed to ngrok as NGROK_AUTHTOKEN. Never written to your shell profile or
          ngrok's config.
        </p>
      </section>

      <section class="py-4" data-testid="ngrok-domain">
        <p :class="SECTION_LABEL">Static domain · optional</p>
        <div class="mt-2.5 flex items-center gap-2">
          <Input
            v-model="domainInput"
            placeholder="name.ngrok-free.app"
            aria-label="ngrok static domain"
            class="flex-1 font-mono"
            :disabled="locked"
          />
          <Button size="sm" :disabled="locked || !domainChanged" @click="saveDomain">
            <Spinner v-if="busy === 'ngrok-domain'" class="size-4" />
            Save
          </Button>
        </div>
        <p :class="HELP">Free plans include one static domain. Leave empty to get a random URL each time.</p>
      </section>
    </div>

    <button
      v-if="hasToken"
      type="button"
      :class="cn(LINK, 'mt-1 text-destructive')"
      :disabled="locked"
      data-action="remove-token"
      @click="emit('remove-token')"
    >
      Remove token
    </button>
  </div>
</template>
