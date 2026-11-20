<script setup lang="ts">
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { isUnderTld, validateDomainShape } from "@/lib/domainValidation";

/**
 * Add a domain to a site or proxy, or edit one (`editing` set). The field
 * resets every time the dialog opens. The daemon is authoritative, so the
 * wrong-TLD notice never blocks Save; only a malformed shape does.
 */
const props = defineProps<{
  open: boolean;
  name: string;
  tld: string;
  /** The domain being edited, or null to add a new one. */
  editing: string | null;
  /** Whether `editing` is the primary domain. */
  editingPrimary?: boolean;
  isWordpress?: boolean;
  busy?: boolean;
}>();

const emit = defineEmits<{ "update:open": [boolean]; submit: [domain: string] }>();

const value = ref("");

watch(
  () => props.open,
  (open) => {
    if (open) value.value = props.editing ?? "";
  },
  { immediate: true },
);

const trimmed = computed(() => value.value.trim());
const shapeError = computed(() =>
  trimmed.value === "" ? null : validateDomainShape(trimmed.value),
);
const tldHint = computed(() =>
  trimmed.value !== "" && shapeError.value === null && !isUnderTld(trimmed.value, props.tld)
    ? `Expected to end in .${props.tld} - the daemon may reject this.`
    : null,
);
const canSave = computed(
  () =>
    !props.busy &&
    trimmed.value !== "" &&
    shapeError.value === null &&
    trimmed.value !== props.editing,
);

function save(): void {
  if (canSave.value) emit("submit", trimmed.value);
}
</script>

<template>
  <Modal
    :open="open"
    :title="editing ? 'Edit domain' : 'Add domain'"
    @update:open="emit('update:open', $event)"
  >
    <label for="add-domain" class="block text-sm font-medium">Domain</label>
    <Input
      id="add-domain"
      v-model="value"
      class="mt-1.5 h-8 font-mono"
      :placeholder="`api.${name}.${tld}`"
      :disabled="busy"
      @keydown.enter="save"
    />
    <InfoBanner v-if="shapeError" variant="destructive" class="mt-3">
      {{ shapeError }}
    </InfoBanner>
    <InfoBanner v-else-if="tldHint" variant="warning" class="mt-3">
      {{ tldHint }}
    </InfoBanner>
    <InfoBanner v-else class="mt-3">
      An exact domain (<code class="font-mono">api.{{ name }}.{{ tld }}</code>) or a single-label
      wildcard (<code class="font-mono">*.{{ name }}.{{ tld }}</code>).
    </InfoBanner>
    <InfoBanner v-if="editingPrimary && isWordpress" class="mt-3">
      This is the primary domain of a WordPress site, so its site URL changes too.
    </InfoBanner>
    <template #footer="{ close }">
      <Button variant="ghost" :disabled="busy" @click="close">Cancel</Button>
      <Button :disabled="!canSave" @click="save">
        <Spinner v-if="busy" class="size-4" />
        {{ editing ? "Save" : "Add domain" }}
      </Button>
    </template>
  </Modal>
</template>
