<script setup lang="ts">
import { ExternalLink } from "lucide-vue-next";

import type { TunnelProvider } from "@/ipc/types";
import { providerName } from "@/lib/providers";
import { CHIP_TONE, cn } from "@/lib/utils";

import { CHIP, LINK } from "./paneStyles";
import ProviderTile from "./ProviderTile.vue";

/** A provider pane's title row: the tile, the name, a readiness chip and the provider's dashboard link. */
defineProps<{
  provider: TunnelProvider;
  /** The readiness chip's text. */
  readiness: string;
  ready: boolean;
  /** The dashboard address, shown without its scheme. */
  url: string;
}>();

const emit = defineEmits<{ "open-url": [url: string] }>();
</script>

<template>
  <div class="flex items-center gap-2.5">
    <ProviderTile :provider="provider" size="md" />
    <h3 class="text-base font-semibold">{{ providerName(provider) }}</h3>
    <span :class="cn(CHIP, ready ? CHIP_TONE.success : CHIP_TONE.warning)" data-testid="pane-readiness">
      {{ readiness }}
    </span>
    <button
      type="button"
      :class="cn(LINK, 'ml-auto inline-flex items-center gap-[5px] text-xs text-brand')"
      @click="emit('open-url', url)"
    >
      {{ url.replace(/^https:\/\//, "") }} <ExternalLink class="size-[11px]" />
    </button>
  </div>
</template>
