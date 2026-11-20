<script setup lang="ts">
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { ResponseHeaderEntry } from "@/ipc/types";

/**
 * Add a response header to a site, or edit one (`editing` set). The fields
 * reset every time the dialog opens. The daemon validates names and values, so
 * the only client-side gate is that both are filled and something changed.
 */
const props = defineProps<{
  open: boolean;
  /** The header being edited, or null to add a new one. */
  editing: ResponseHeaderEntry | null;
  busy?: boolean;
}>();

const emit = defineEmits<{ "update:open": [boolean]; submit: [name: string, value: string] }>();

const name = ref("");
const value = ref("");

watch(
  () => props.open,
  (open) => {
    if (!open) return;
    name.value = props.editing?.name ?? "";
    value.value = props.editing?.value ?? "";
  },
  { immediate: true },
);

const canSave = computed(() => {
  const n = name.value.trim();
  const v = value.value.trim();
  if (props.busy || n === "" || v === "") return false;
  return !props.editing || n !== props.editing.name || v !== props.editing.value;
});

function save(): void {
  if (canSave.value) emit("submit", name.value.trim(), value.value.trim());
}
</script>

<template>
  <Modal
    :open="open"
    :title="editing ? 'Edit response header' : 'Add response header'"
    @update:open="emit('update:open', $event)"
  >
    <label for="header-name" class="block text-sm font-medium">Name</label>
    <Input
      id="header-name"
      v-model="name"
      class="mt-1.5 h-8 font-mono"
      placeholder="Access-Control-Allow-Origin"
      :disabled="busy"
      @keydown.enter="save"
    />
    <label for="header-value" class="mt-3 block text-sm font-medium">Value</label>
    <Input
      id="header-value"
      v-model="value"
      class="mt-1.5 h-8 font-mono"
      placeholder="https://app.test"
      :disabled="busy"
      @keydown.enter="save"
    />
    <InfoBanner class="mt-3">
      Added to every response for this site, including errors and redirects, but not to responses
      from proxy path rules. A header with the same name sent by the app is replaced.
    </InfoBanner>
    <template #footer="{ close }">
      <Button variant="ghost" :disabled="busy" @click="close">Cancel</Button>
      <Button :disabled="!canSave" @click="save">
        <Spinner v-if="busy" class="size-4" />
        {{ editing ? "Save" : "Add header" }}
      </Button>
    </template>
  </Modal>
</template>
