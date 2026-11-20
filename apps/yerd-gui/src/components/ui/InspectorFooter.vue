<script setup lang="ts">
import { inject } from "vue";

import { INSPECTOR_FOOTER_KEY } from "@/components/ui/inspectorFooter";

/**
 * An inspector's action bar. Inside an `InspectorFrame` it is teleported to the
 * frame's bar below the scroll area, so its buttons stay in reach on long pages
 * and the scrollbar stops above it; elsewhere it renders in place. `active`
 * hides it while its panel is hidden with `v-show`. The `start` slot holds a
 * note on the left; the default slot holds the buttons.
 */
withDefaults(defineProps<{ active?: boolean }>(), { active: true });

const target = inject(INSPECTOR_FOOTER_KEY, null);
</script>

<template>
  <Teleport defer :to="target ?? 'body'" :disabled="!target">
    <div
      v-show="active"
      class="flex items-center justify-end gap-2 border-t bg-background px-7 py-3"
      data-testid="inspector-footer"
    >
      <span v-if="$slots.start" class="mr-auto min-w-0 truncate text-xs text-faint">
        <slot name="start" />
      </span>
      <slot />
    </div>
  </Teleport>
</template>
