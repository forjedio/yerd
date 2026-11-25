<script setup lang="ts">
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { TunnelProviderStatus } from "@/ipc/types";
import { readinessLabel } from "@/lib/providers";
import { cn } from "@/lib/utils";

import BinarySection from "./BinarySection.vue";
import PaneHeader from "./PaneHeader.vue";
import { FIELD, FOOTER, HELP, LABEL, LINK, PANE, SECTION } from "./paneStyles";

/**
 * The ngrok pane of the Tunnel providers sheet: the binary, the authtoken
 * (only ever shown masked; saved, replaced or removed through the dialog) and
 * the optional static domain. Presentational: every action is emitted. The
 * default slot fills the right of the bottom row (the sheet's Done).
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

</script>

<template>
  <div :class="PANE">
    <PaneHeader
      provider="ngrok"
      :readiness="readinessLabel(status.readiness)"
      :ready="ready"
      url="https://dashboard.ngrok.com"
      @open-url="(url: string) => emit('open-url', url)"
    />

    <BinarySection
      :status="status"
      name="ngrok"
      bin="ngrok"
      :locked="locked"
      :connected="connected"
      @tooling="emit('tooling')"
      @recheck="emit('recheck')"
    />

    <section :class="SECTION" data-testid="ngrok-token">
      <p :class="LABEL">Authtoken</p>
      <div v-if="!editingToken" class="flex items-center gap-2">
        <p :class="cn(FIELD, 'flex flex-1 items-center border px-[11px] font-semibold')" data-testid="token-hint">
          {{ status.authtoken_hint }}
        </p>
        <Button size="sm" variant="outline" :disabled="locked" @click="replacing = true">Replace</Button>
      </div>
      <div v-else class="flex items-center gap-2">
        <Input
          v-model="tokenInput"
          type="password"
          placeholder="Paste your ngrok authtoken"
          aria-label="ngrok authtoken"
          :class="cn(FIELD, 'flex-1')"
          :disabled="locked"
          @keydown.enter="saveToken"
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

    <section :class="SECTION" data-testid="ngrok-domain">
      <p :class="LABEL">Static domain · optional</p>
      <div class="flex items-center gap-2">
        <Input
          v-model="domainInput"
          placeholder="name.ngrok-free.app"
          aria-label="ngrok static domain"
          :class="cn(FIELD, 'flex-1')"
          :disabled="locked"
          @keydown.enter="domainChanged && saveDomain()"
        />
        <Button v-if="domainChanged" size="sm" :disabled="locked" @click="saveDomain">
          <Spinner v-if="busy === 'ngrok-domain'" class="size-4" />
          Save
        </Button>
      </div>
      <p :class="HELP">Free plans include one static domain. Leave empty to use the address ngrok assigns to your account.</p>
    </section>

    <div :class="FOOTER">
      <button
        v-if="hasToken"
        type="button"
        :class="cn(LINK, 'text-destructive')"
        :disabled="locked"
        data-action="remove-token"
        @click="emit('remove-token')"
      >
        Remove token
      </button>
      <span v-else />
      <slot />
    </div>
  </div>
</template>
