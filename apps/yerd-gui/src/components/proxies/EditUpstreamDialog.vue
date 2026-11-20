<script setup lang="ts">
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import { hasScheme, normalizeUpstream, upstreamLabel } from "@/lib/proxies";

/**
 * Edit upstream: changes where a whole-domain proxy or a path rule forwards to,
 * using the same `http://` input and validation as New proxy. The field resets
 * to the current upstream on each open. Save stays disabled until the
 * normalised input differs from the current upstream. Submitting closes the
 * dialog, then emits the normalised URL.
 */
const props = defineProps<{
  open: boolean;
  /** The proxy's host or the rule's `site.tld/prefix`, shown as context. */
  host: string;
  /** The current upstream URL. */
  current: string;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  save: [url: string];
}>();

const url = ref("");

watch(
  () => props.open,
  (open) => {
    if (open) url.value = upstreamLabel(props.current);
  },
  { immediate: true },
);

const showScheme = computed(() => !hasScheme(url.value));
const upstream = computed(() => normalizeUpstream(url.value));
const valid = computed(() => url.value.trim() !== "");
const changed = computed(() => upstream.value !== normalizeUpstream(props.current));
const canSave = computed(() => valid.value && changed.value);

function submit(): void {
  if (!canSave.value) return;
  emit("update:open", false);
  emit("save", upstream.value);
}
</script>

<template>
  <DialogFrame :open="open" title="Edit upstream" width="md" @update:open="emit('update:open', $event)">
    <p class="truncate font-mono text-[13px] text-muted-foreground" data-testid="edit-upstream-host">
      {{ host }}
    </p>

    <label class="mt-5 block text-[13px] font-semibold" for="edit-upstream-url">Forward to</label>
    <div
      class="mt-2 flex h-10 cursor-text items-center rounded-lg border bg-background px-3 font-mono text-sm transition-colors focus-within:border-brand focus-within:ring-1 focus-within:ring-brand"
    >
      <span v-if="showScheme" class="mr-1 shrink-0 text-faint" data-testid="scheme-hint">http://</span>
      <input
        id="edit-upstream-url"
        v-model="url"
        class="min-w-0 flex-1 bg-transparent outline-none placeholder:text-faint"
        placeholder="localhost:3000"
        spellcheck="false"
        autocomplete="off"
        @keydown.enter.prevent="submit"
      />
    </div>

    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Cancel</Button>
      <Button :disabled="!canSave" @click="submit">Save</Button>
    </template>
  </DialogFrame>
</template>
