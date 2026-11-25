<script setup lang="ts">
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { TunnelProviderStatus } from "@/ipc/types";
import { readinessLabel } from "@/lib/providers";
import { CHIP_TONE, cn } from "@/lib/utils";

import BinarySection from "./BinarySection.vue";
import PaneHeader from "./PaneHeader.vue";
import { BOX, CHIP, FIELD, FOOTER, HELP, LABEL, PANE, SECTION } from "./paneStyles";

/**
 * The zrok pane of the Tunnel providers sheet: the binary, and enabling this
 * machine with a zrok account token (the daemon runs `zrok2 enable` in its own
 * zrok environment) or disabling it again. Presentational: every action is
 * emitted. The default slot fills the right of the bottom row (the sheet's Done).
 */
const props = defineProps<{
  status: TunnelProviderStatus;
  /** The dialog's in-flight action key (`zrok` while enabling or disabling). */
  busy: string | null;
  connected: boolean;
}>();

const emit = defineEmits<{
  tooling: [];
  recheck: [];
  "open-url": [url: string];
  enable: [token: string];
  disable: [];
}>();

const tokenInput = ref("");

watch(
  () => props.status.configured,
  () => {
    tokenInput.value = "";
  },
);

const locked = computed(() => props.busy !== null || !props.connected);
const installed = computed(() => props.status.readiness !== "not_installed");
const ready = computed(() => props.status.readiness === "ready");

function enable(): void {
  const token = tokenInput.value.trim();
  if (token) emit("enable", token);
}

</script>

<template>
  <div :class="PANE">
    <PaneHeader
      provider="zrok"
      :readiness="readinessLabel(status.readiness)"
      :ready="ready"
      url="https://zrok.io"
      @open-url="(url: string) => emit('open-url', url)"
    />

    <BinarySection
      :status="status"
      name="zrok"
      bin="zrok2"
      :locked="locked"
      :connected="connected"
      @tooling="emit('tooling')"
      @recheck="emit('recheck')"
    />

    <section :class="SECTION" data-testid="zrok-machine">
      <p :class="LABEL">Enable this machine</p>
      <template v-if="status.configured">
        <div :class="BOX">
          <p class="flex-1 text-[13px] font-semibold">This machine</p>
          <span :class="cn(CHIP, CHIP_TONE.success)" data-testid="zrok-enabled">
            <span class="size-1.5 rounded-full bg-success" aria-hidden="true" /> Enabled
          </span>
          <Button size="sm" variant="outline" :disabled="locked" data-action="disable" @click="emit('disable')">
            <Spinner v-if="busy === 'zrok'" class="size-4" />
            Disable
          </Button>
        </div>
        <p :class="HELP">Disabling releases this machine from your zrok account. Enable again with any account token.</p>
      </template>
      <template v-else>
        <div class="flex items-center gap-2">
          <Input
            v-model="tokenInput"
            type="password"
            placeholder="Paste your zrok account token"
            aria-label="zrok account token"
            :class="cn(FIELD, 'flex-1')"
            :disabled="locked"
            @keydown.enter="installed && enable()"
          />
          <Button size="sm" :disabled="locked || !installed || !tokenInput.trim()" data-action="enable" @click="enable">
            <Spinner v-if="busy === 'zrok'" class="size-4" />
            Enable
          </Button>
        </div>
        <p :class="HELP">
          zrok is enabled once per machine. Yerd runs zrok2 enable with this token in its own zrok environment,
          separate from ~/.zrok2.
        </p>
      </template>
    </section>

    <div :class="FOOTER">
      <span />
      <slot />
    </div>
  </div>
</template>
