<script setup lang="ts">
import { computed } from "vue";

import Button from "@/components/ui/Button.vue";
import { SECTION_LABEL } from "@/components/ui/styles";
import type { TunnelProviderStatus } from "@/ipc/types";
import { PROVIDER_ORDER, providerCardDot, providerCardStatus, providerName } from "@/lib/providers";
import { cn } from "@/lib/utils";

import ProviderTile from "./providers/ProviderTile.vue";

/**
 * The Share page's PROVIDERS summary: one card per provider with its tile,
 * name and setup status, and a Manage link that opens the Tunnel providers
 * sheet. A provider missing from `providers` (not loaded yet) is left out.
 * When the list failed to load, `error` replaces the cards with a Retry.
 */
const props = defineProps<{ providers: TunnelProviderStatus[]; error?: string | null }>();

const emit = defineEmits<{ manage: []; retry: [] }>();

const cards = computed(() =>
  PROVIDER_ORDER.flatMap((provider) => {
    const status = props.providers.find((s) => s.provider === provider);
    return status ? [status] : [];
  }),
);
</script>

<template>
  <div data-testid="provider-cards">
    <div class="flex items-center justify-between pb-2 pr-4">
      <span :class="SECTION_LABEL">Providers</span>
      <button
        type="button"
        class="text-xs font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        data-testid="providers-manage"
        @click="emit('manage')"
      >
        Manage
      </button>
    </div>
    <div
      v-if="error"
      class="mr-4 flex items-center gap-3 rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2.5"
      data-testid="providers-error"
    >
      <div class="min-w-0 flex-1">
        <p class="text-[13px] font-semibold">Couldn't load tunnel providers</p>
        <p class="truncate text-[11px] text-muted-foreground">{{ error }}</p>
      </div>
      <Button variant="outline" size="sm" data-testid="providers-retry" @click="emit('retry')">Retry</Button>
    </div>
    <div v-else class="grid grid-cols-3 gap-2 pr-4">
      <div
        v-for="status in cards"
        :key="status.provider"
        class="flex min-w-0 flex-col gap-1.5 rounded-lg border bg-card p-2.5"
        :data-testid="`provider-card-${status.provider}`"
      >
        <ProviderTile :provider="status.provider" />
        <span class="truncate text-[13px] font-semibold">{{ providerName(status.provider) }}</span>
        <span class="flex items-center gap-1.5 text-[11px] text-muted-foreground">
          <span :class="cn('size-1.5 shrink-0 rounded-full', providerCardDot(status))" aria-hidden="true" />
          <span class="truncate" data-testid="provider-card-status">{{ providerCardStatus(status) }}</span>
        </span>
      </div>
    </div>
  </div>
</template>
