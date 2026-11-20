<script setup lang="ts">
import { Copy } from "lucide-vue-next";
import { ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import { useToast } from "@/composables/useToast";
import { getDiagnostics, IpcError } from "@/ipc/client";

/**
 * The diagnostics snapshot from About ▸ Troubleshooting, gathered fresh each
 * time the dialog opens, to copy into a bug report.
 */
const props = defineProps<{ open: boolean }>();

const emit = defineEmits<{ "update:open": [boolean] }>();

const toast = useToast();
const diagText = ref("");
const diagLoading = ref(false);

async function load(): Promise<void> {
  diagLoading.value = true;
  diagText.value = "";
  try {
    diagText.value = await getDiagnostics();
  } catch (e) {
    toast.error("Couldn't gather diagnostics", (e as IpcError).message);
    diagText.value = "";
  } finally {
    diagLoading.value = false;
  }
}

watch(
  () => props.open,
  (open) => {
    if (open) void load();
  },
  { immediate: true },
);

async function copyDiagnostics(): Promise<void> {
  try {
    await navigator.clipboard.writeText(diagText.value);
    toast.success("Copied diagnostics", "Paste this when reporting the problem.");
  } catch {
    toast.error("Couldn't copy", "Your browser blocked clipboard access.");
  }
}
</script>

<template>
  <DialogFrame
    :open="open"
    title="Diagnostics"
    width="xl"
    @update:open="emit('update:open', $event)"
  >
    <p class="mb-3 text-[13px] text-muted-foreground">
      A snapshot of paths, service configuration, and recent errors.
    </p>
    <pre
      data-testid="diagnostics"
      class="h-[min(32rem,65vh)] overflow-auto whitespace-pre-wrap break-all rounded-lg border bg-sidebar p-3.5 font-mono text-[11px] leading-[20px] text-muted-foreground"
    ><span v-if="diagLoading || !diagText" class="text-faint">{{ diagLoading ? "Gathering…" : "No diagnostics available." }}</span><template v-else>{{ diagText }}</template></pre>

    <template #footer>
      <Button
        variant="outline"
        :disabled="diagLoading || !diagText"
        @click="copyDiagnostics"
      >
        <Copy /> Copy
      </Button>
      <Button variant="outline" @click="emit('update:open', false)">Close</Button>
    </template>
  </DialogFrame>
</template>
