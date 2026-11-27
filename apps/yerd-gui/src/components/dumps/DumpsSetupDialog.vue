<script setup lang="ts">
import { computed } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Switch from "@/components/ui/Switch.vue";
import { SECTION_LABEL } from "@/components/ui/styles";
import type { DumpsBusy } from "@/composables/useDumps";
import type { DumpExtStatus, DumpsStatusResponse } from "@/ipc/types";
import { DUMP_SIGNALS } from "@/lib/dumps";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * Dump setup, opened from the Dumps toolbar: the interception switch, the dump
 * server address with a link to Settings ▸ Application Ports where the port
 * lives, the eight captured-signal switches (disabled while interception is
 * off) and the per-version PHP extension status.
 */
const props = defineProps<{
  open: boolean;
  status: DumpsStatusResponse | null;
  busy: DumpsBusy;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  setEnabled: [next: boolean];
  setFeature: [key: string, next: boolean];
  changePort: [];
}>();

const enabled = computed(() => props.status?.enabled ?? false);
const extensions = computed(() => props.status?.extensions ?? []);

function featureOn(key: string): boolean {
  return props.status?.features?.[key] ?? true;
}

function extChip(ext: DumpExtStatus): { text: string; class: string } {
  if (ext.legacy) return { text: "TOO OLD — NO DUMPS", class: CHIP_TONE.warning };
  if (ext.present) return { text: "INSTALLED", class: CHIP_TONE.success };
  return { text: "NOT INSTALLED", class: CHIP_TONE.warning };
}

</script>

<template>
  <DialogFrame :open="open" title="Dump setup" width="lg" @update:open="emit('update:open', $event)">
    <div class="flex items-start gap-4">
      <div class="min-w-0 flex-1">
        <p class="text-sm font-semibold">Intercept dumps</p>
        <p class="mt-0.5 text-xs text-muted-foreground">Streams dump() and Laravel telemetry from every site.</p>
      </div>
      <Switch
        class="mt-0.5"
        :model-value="enabled"
        :disabled="!status || busy === 'enabled'"
        aria-label="Intercept dumps"
        @update:model-value="emit('setEnabled', $event)"
      />
    </div>

    <div class="mt-4 flex items-end gap-4">
      <div class="min-w-0 flex-1">
        <p class="text-sm font-semibold">Dump server</p>
        <p class="mt-0.5 text-xs text-muted-foreground" data-testid="dump-server-address">
          <span class="font-mono text-foreground">127.0.0.1:{{ status?.port ?? 2304 }}</span>
          · {{ status?.running ? "running" : "stopped" }}
        </p>
      </div>
      <button
        type="button"
        class="shrink-0 text-xs font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        @click="emit('changePort')"
      >
        Change port
      </button>
    </div>

    <div class="mt-5 border-t pt-4">
      <p :class="SECTION_LABEL">Captured signals</p>
      <div :class="cn('mt-3 grid gap-x-6 gap-y-3 sm:grid-cols-2 transition-opacity', !enabled && 'opacity-60')">
        <div v-for="signal in DUMP_SIGNALS" :key="signal.key" class="flex items-center justify-between gap-4">
          <span class="text-[13px]">{{ signal.label }}</span>
          <Switch
            :model-value="featureOn(signal.key)"
            :disabled="!enabled || busy === `feature:${signal.key}`"
            :aria-label="signal.label"
            @update:model-value="emit('setFeature', signal.key, $event)"
          />
        </div>
      </div>
      <p v-if="status && !enabled" class="mt-3 text-xs text-muted-foreground">
        Turn on Intercept dumps to change these.
      </p>
    </div>

    <div class="mt-5 border-t pt-4">
      <p :class="SECTION_LABEL">PHP extension</p>
      <p v-if="extensions.length === 0" class="mt-3 text-[13px] text-muted-foreground">No PHP versions installed.</p>
      <ul v-else class="mt-2 flex flex-col">
        <li
          v-for="ext in extensions"
          :key="ext.version"
          class="flex h-[30px] items-center justify-between gap-4"
          :data-version="ext.version"
        >
          <span class="font-mono text-[13px]">PHP {{ ext.version }}</span>
          <span
            :class="cn('inline-flex h-5 items-center rounded px-1.5 text-[11px] font-semibold', extChip(ext).class)"
            data-testid="ext-chip"
          >
            {{ extChip(ext).text }}
          </span>
        </li>
      </ul>
    </div>

    <template #footer>
      <Button @click="emit('update:open', false)">Done</Button>
    </template>
  </DialogFrame>
</template>
