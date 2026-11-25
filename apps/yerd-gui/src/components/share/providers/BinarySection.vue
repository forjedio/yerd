<script setup lang="ts">
import { computed } from "vue";
import { Download, RefreshCw } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import type { TunnelProviderStatus } from "@/ipc/types";
import { binaryLine, needsTooling, sourceLabel } from "@/lib/providers";
import { CHIP_TONE, cn } from "@/lib/utils";

import { BOX, CHIP, HELP, LABEL, SECTION } from "./paneStyles";

/**
 * The BINARY section shared by the ngrok and zrok panes: which binary is in
 * use (Yerd's copy or one found on PATH, with its path), a Re-check, and a
 * link to Tooling when there is something to install or update there. A
 * missing binary is drawn as an amber callout.
 */
const props = defineProps<{
  status: TunnelProviderStatus;
  /** The provider's display name, used in the install hint. */
  name: string;
  /** The binary's command name (`ngrok`, `zrok2`). */
  bin: string;
  locked: boolean;
  connected: boolean;
}>();

const emit = defineEmits<{ tooling: []; recheck: [] }>();

const installed = computed(() => props.status.readiness !== "not_installed");
const update = computed(() => (props.status.source === "managed" ? (props.status.update_available ?? null) : null));
</script>

<template>
  <section :class="SECTION" data-testid="provider-binary">
    <p :class="LABEL">Binary</p>
    <div v-if="installed" :class="BOX">
      <div class="min-w-0 flex-1">
        <p class="flex items-center gap-2 text-[13px] font-semibold">
          {{ binaryLine(status, bin) }}
          <span :class="cn(CHIP, CHIP_TONE.success)" data-testid="binary-source">{{ sourceLabel(status) }}</span>
        </p>
        <p v-if="status.path" class="mt-[3px] truncate font-mono text-[11.5px] text-muted-foreground" data-testid="binary-path">
          {{ status.path }}
        </p>
      </div>
      <Button size="sm" variant="outline" :disabled="locked" data-action="recheck" @click="emit('recheck')">
        <RefreshCw /> Re-check
      </Button>
      <Button
        v-if="needsTooling(status)"
        size="sm"
        variant="outline"
        :disabled="!connected"
        data-action="tooling"
        @click="emit('tooling')"
      >
        <Download /> Update
      </Button>
    </div>
    <div v-else class="flex flex-col gap-3 rounded-[9px] border border-warning/50 bg-warning/10 p-3.5">
      <div>
        <p class="text-[13px] font-semibold">{{ name }} isn't installed</p>
        <p :class="cn(HELP, 'mt-1')">
          Install it from Tooling, then re-check. Yerd also uses a {{ bin }} it finds on your PATH.
        </p>
      </div>
      <div class="flex items-center gap-2">
        <Button size="sm" variant="outline" :disabled="locked" data-action="recheck" @click="emit('recheck')">
          <RefreshCw /> Re-check
        </Button>
        <Button size="sm" :disabled="!connected" data-action="tooling" @click="emit('tooling')">Open Tooling</Button>
      </div>
    </div>
    <p v-if="update" :class="HELP">Update to {{ update }} available in Tooling.</p>
  </section>
</template>
