<script setup lang="ts">
import { ref, watch } from "vue";
import { FolderOpen } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { useToast } from "@/composables/useToast";
import {
  addPhpExtension,
  IpcError,
  type PhpExtensionsMap,
  pickExtensionFile,
} from "@/ipc/client";
import type { PhpVersion } from "@/ipc/types";

const props = defineProps<{
  open: boolean;
  /** The version the extension is registered against (the active tab). */
  version: PhpVersion;
}>();

const emit = defineEmits<{
  (e: "update:open", open: boolean): void;
  /** Fired with the daemon's refreshed extension map after a successful add. */
  (e: "added", map: PhpExtensionsMap): void;
}>();

const toast = useToast();
const path = ref("");
const name = ref("");
const zend = ref(false);
const busy = ref(false);
// The daemon load-probes the .so, so a failure here is about the file the user
// just chose. Keep it beside the field rather than in a toast that outlives it.
const problem = ref<string | null>(null);

watch(
  () => props.open,
  (isOpen) => {
    if (!isOpen) return;
    path.value = "";
    name.value = "";
    zend.value = false;
    problem.value = null;
  },
);

async function browse(): Promise<void> {
  const picked = await pickExtensionFile();
  if (picked === null) return;
  path.value = picked;
  problem.value = null;
}

async function add(): Promise<void> {
  const chosen = path.value.trim();
  if (!chosen) {
    problem.value = "choose an extension file";
    return;
  }
  busy.value = true;
  problem.value = null;
  try {
    const map = await addPhpExtension(
      props.version,
      chosen,
      zend.value,
      name.value.trim() || undefined,
    );
    toast.success("Extension registered", "Loaded into the FPM pool and CLI.");
    emit("added", map);
    emit("update:open", false);
  } catch (e) {
    problem.value = (e as IpcError).message;
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <Modal
    :open="open"
    :title="`Add extension to PHP ${version}`"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <div class="flex flex-col gap-4">
      <div>
        <label class="text-xs font-medium" for="ext-path">Extension path</label>
        <div class="mt-1 flex items-start gap-2">
          <Input
            id="ext-path"
            v-model="path"
            placeholder="/opt/homebrew/lib/php/pecl/20250925/scrypt.so"
            class="flex-1"
          />
          <Button variant="outline" @click="browse">
            <FolderOpen class="size-4" />
            Browse…
          </Button>
        </div>
        <p v-if="problem" class="mt-1 text-xs text-destructive">{{ problem }}</p>
      </div>

      <div>
        <label class="text-xs font-medium" for="ext-name">Name (optional)</label>
        <Input
          id="ext-name"
          v-model="name"
          placeholder="defaults to the .so filename"
          class="mt-1"
        />
      </div>

      <label class="flex items-center gap-2 text-sm">
        <Switch v-model="zend" aria-label="Load as a Zend extension" />
        Zend extension
      </label>
    </div>

    <template #footer="{ close }">
      <Button variant="ghost" size="sm" @click="close">Cancel</Button>
      <Button size="sm" :disabled="busy" @click="add">
        <Spinner v-if="busy" class="size-4" />
        {{ busy ? "Checking…" : "Add extension" }}
      </Button>
    </template>
  </Modal>
</template>
