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
 * The zrok pane of the Tunnel providers sheet: the binary, and enabling this
 * machine with a zrok account token (the daemon runs `zrok2 enable` in its own
 * zrok environment) or disabling it again. Presentational: every action is
 * emitted.
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

const CHIP = "inline-flex shrink-0 items-center gap-1.5 rounded px-2 py-0.5 text-[11px] font-semibold uppercase";
const LINK =
  "shrink-0 rounded text-xs font-medium hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50";
const HELP = "mt-2 text-xs text-muted-foreground";
</script>

<template>
  <div>
    <div class="mb-4 flex items-center gap-2.5">
      <ProviderTile provider="zrok" size="md" />
      <h3 class="text-[15px] font-semibold">zrok</h3>
      <span :class="cn(CHIP, ready ? CHIP_TONE.success : CHIP_TONE.warning)" data-testid="pane-readiness">
        {{ readinessLabel(status.readiness) }}
      </span>
      <button
        type="button"
        :class="cn(LINK, 'ml-auto inline-flex items-center gap-1 text-brand')"
        @click="emit('open-url', 'https://zrok.io')"
      >
        zrok.io <ExternalLink class="size-3" />
      </button>
    </div>

    <div class="divide-y">
      <BinarySection
        :status="status"
        name="zrok"
        bin="zrok2"
        :locked="locked"
        :connected="connected"
        @tooling="emit('tooling')"
        @recheck="emit('recheck')"
      />

      <section class="py-4" data-testid="zrok-machine">
        <p :class="SECTION_LABEL">Enable this machine</p>
        <template v-if="status.configured">
          <div class="mt-2.5 flex items-center gap-3">
            <span :class="cn(CHIP, CHIP_TONE.success)" data-testid="zrok-enabled">
              <span class="size-1.5 rounded-full bg-success" aria-hidden="true" /> Enabled
            </span>
            <Button class="ml-auto" size="sm" variant="outline" :disabled="locked" data-action="disable" @click="emit('disable')">
              <Spinner v-if="busy === 'zrok'" class="size-4" />
              Disable
            </Button>
          </div>
          <p :class="HELP">
            Disabling releases this machine from your zrok account. Enable again with any account token.
          </p>
        </template>
        <template v-else>
          <div class="mt-2.5 flex items-center gap-2">
            <Input
              v-model="tokenInput"
              type="password"
              placeholder="Paste your zrok account token"
              aria-label="zrok account token"
              class="flex-1"
              :disabled="locked"
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
    </div>
  </div>
</template>
