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
        'inline-flex shrink-0 items-center justify-center font-mono font-bold',
        size === 'sm' ? 'size-[26px] rounded-md text-[10px]' : 'size-[30px] rounded-[7px] text-xs',
        provider === 'cloudflare' && 'bg-[#F38020] text-white',
        provider === 'ngrok' && 'border bg-secondary text-foreground',
        provider === 'zrok' && 'bg-violet-600 text-white',
      )
    "
  >
    <Cloud v-if="provider === 'cloudflare'" class="size-3.5" />
    <template v-else-if="provider === 'ngrok'">ng</template>
    <template v-else>z</template>
  </span>
</template>
