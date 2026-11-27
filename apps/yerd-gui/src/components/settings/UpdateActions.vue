<script setup lang="ts">
import { Download, RefreshCw } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { UpdateChannel } from "@/ipc/types";

/**
 * The About section's update controls, shared by both of its layouts: the
 * release-channel switch, then Update when one is available or Check now
 * otherwise. Everything is disabled while the daemon is down.
 */
defineProps<{
  channel: UpdateChannel;
  running: boolean;
  available: boolean;
  applying: boolean;
  checking: boolean;
}>();

const emit = defineEmits<{ channel: [string]; update: []; check: [] }>();

const channelOptions = [
  { value: "stable", label: "Stable" },
  { value: "edge", label: "Edge" },
] as const;
</script>

<template>
  <div class="flex items-center gap-2">
    <SegmentedControl
      :model-value="channel"
      :options="channelOptions"
      size="md"
      variant="plain"
      aria-label="Update channel"
      :disabled="!running"
      @update:model-value="emit('channel', $event)"
    />
    <Button
      v-if="available"
      size="sm"
      :disabled="!running || applying"
      @click="emit('update')"
    >
      <Spinner v-if="applying" class="size-4" />
      <Download v-else />
      {{ applying ? "Updating…" : "Update" }}
    </Button>
    <Button
      v-else
      variant="outline"
      size="sm"
      :disabled="!running || checking"
      @click="emit('check')"
    >
      <Spinner v-if="checking" class="size-4" />
      <RefreshCw v-else />
      Check now
    </Button>
  </div>
</template>
