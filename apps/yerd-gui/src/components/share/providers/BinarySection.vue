<script setup lang="ts">
import { computed } from "vue";
import { RefreshCw } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import { SECTION_LABEL } from "@/components/ui/styles";
import type { TunnelProviderStatus } from "@/ipc/types";
import { binaryLine, needsTooling, sourceLabel } from "@/lib/providers";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * The BINARY section shared by the ngrok and zrok panes: which binary is in
 * use (Yerd's copy or one found on PATH, with its path), a Re-check, and a
 * link to Tooling when there is something to install or update there.
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

const CHIP = "inline-flex shrink-0 items-center rounded px-2 py-0.5 text-[11px] font-semibold uppercase";
</script>

<template>
  <section class="py-4 first:pt-0" data-testid="provider-binary">
    <p :class="SECTION_LABEL">Binary</p>
    <div class="mt-2.5 flex items-center gap-3 rounded-lg border px-3.5 py-3">
      <div class="min-w-0 flex-1">
        <template v-if="installed">
          <p class="flex items-center gap-2 text-[13px] font-semibold">
            {{ binaryLine(status, bin) }}
            <span :class="cn(CHIP, CHIP_TONE.success)" data-testid="binary-source">{{ sourceLabel(status) }}</span>
          </p>
          <p v-if="status.path" class="truncate font-mono text-[11px] text-faint" data-testid="binary-path">
            {{ status.path }}
          </p>
          <p v-else-if="update" class="text-xs text-muted-foreground">Update to {{ update }} available in Tooling</p>
        </template>
        <template v-else>
          <p class="text-[13px] font-semibold">Not installed</p>
          <p class="text-xs text-muted-foreground">Install {{ name }} from Tooling.</p>
        </template>
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
        Open Tooling
      </Button>
    </div>
  </section>
</template>
