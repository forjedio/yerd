<script setup lang="ts">
import { Cloud } from "lucide-vue-next";

import type { TunnelProvider } from "@/ipc/types";
import { cn } from "@/lib/utils";

/** A provider's small square icon tile: Cloudflare's cloud, ngrok's `ng`, zrok's `z`. */
withDefaults(defineProps<{ provider: TunnelProvider; size?: "sm" | "md" }>(), { size: "sm" });
</script>

<template>
  <span
    aria-hidden="true"
    :class="
      cn(
        'inline-flex shrink-0 items-center justify-center rounded-md text-[10px] font-bold',
        size === 'sm' ? 'size-6' : 'size-7',
        provider === 'cloudflare' && 'bg-orange-500 text-white',
        provider === 'ngrok' && 'bg-muted text-foreground',
        provider === 'zrok' && 'bg-violet-600 text-white',
      )
    "
  >
    <Cloud v-if="provider === 'cloudflare'" class="size-3.5" />
    <template v-else-if="provider === 'ngrok'">ng</template>
    <template v-else>z</template>
  </span>
</template>
