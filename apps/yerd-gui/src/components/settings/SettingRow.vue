<script setup lang="ts">
/**
 * A Settings row: the label and its description on the left, the control on
 * the right. `descriptionId` lets the control point `aria-describedby` at the
 * description.
 */
defineProps<{ label: string; descriptionId?: string }>();
</script>

<template>
  <div class="flex min-h-[50px] items-center gap-4 px-[18px] py-1.5">
    <div class="min-w-0 flex-1">
      <p class="text-[13.5px] font-semibold">{{ label }}</p>
      <p
        v-if="$slots.description"
        :id="descriptionId"
        class="mt-0.5 text-[12.5px] text-muted-foreground"
      >
        <slot name="description" />
      </p>
    </div>
    <div class="flex shrink-0 items-center gap-2">
      <slot name="control" />
    </div>
  </div>
</template>
