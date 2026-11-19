<script setup lang="ts">
import { Copy, FolderOpen } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import { useToast } from "@/composables/useToast";
import { IpcError, pickDirectory } from "@/ipc/client";
import type { SiteEntry } from "@/ipc/types";

/**
 * Change a site's served web root, relative to the site folder. An empty field
 * resets it to auto-detection. The edit buffer resets every time the dialog
 * opens, so an unsaved edit never carries across to the next open or site.
 */
const props = defineProps<{
  open: boolean;
  site: SiteEntry;
  busy?: boolean;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  changeWebRoot: [site: SiteEntry, path: string];
}>();

const toast = useToast();
const webRoot = ref("");
const changed = computed(() => !props.busy && webRoot.value.trim() !== (props.site.web_subpath ?? ""));

watch(
  [() => props.open, () => props.site.name],
  () => {
    webRoot.value = props.site.web_subpath ?? "";
  },
  { immediate: true },
);

function relativeWebRoot(siteRoot: string, selectedDirectory: string): string | null {
  const root = siteRoot.replace(/[\\/]+$/, "").replace(/[\\]/g, "/");
  const selected = selectedDirectory.replace(/[\\]/g, "/");
  if (root === "/") return selected.startsWith("/") ? selected.slice(1) : null;
  if (selected === root) return "";
  return selected.startsWith(`${root}/`) ? selected.slice(root.length + 1) : null;
}

async function chooseWebRoot(): Promise<void> {
  try {
    const directory = await pickDirectory(props.site.document_root);
    if (!directory) return;
    const relative = relativeWebRoot(props.site.document_root, directory);
    if (relative === null) {
      toast.error("Invalid web root", "Choose a directory inside the site folder.");
      return;
    }
    webRoot.value = relative;
  } catch (error) {
    toast.error("Couldn't choose web root", (error as IpcError).message);
  }
}

async function copyWebRoot(): Promise<void> {
  try {
    await navigator.clipboard.writeText(webRoot.value);
    toast.success("Copied web root");
  } catch {
    toast.error("Couldn't copy web root", "Your browser blocked clipboard access.");
  }
}

function save(close: () => void): void {
  emit("changeWebRoot", props.site, webRoot.value.trim());
  close();
}
</script>

<template>
  <Modal :open="open" title="Change web root" @update:open="emit('update:open', $event)">
    <label class="block text-sm font-medium" for="site-web-root">Web root</label>
    <div class="mt-1.5 flex gap-2">
      <Input
        id="site-web-root"
        v-model="webRoot"
        class="h-8 min-w-0 font-mono"
        :disabled="busy"
        placeholder="public"
        aria-label="Site web root"
        @keydown.enter="changed && save(() => emit('update:open', false))"
      />
      <Button
        variant="outline"
        :disabled="busy"
        aria-label="Choose web root directory"
        @click="chooseWebRoot"
      >
        <FolderOpen class="size-4" />
      </Button>
      <Button
        variant="outline"
        :disabled="busy"
        aria-label="Copy web root"
        title="Copy web root"
        @click="copyWebRoot"
      >
        <Copy class="size-4" />
      </Button>
    </div>
    <InfoBanner class="mt-3">
      Directory served as the document root, relative to the site folder (e.g.
      <code class="font-mono">public</code>). Leave blank to auto-detect.
    </InfoBanner>
    <template #footer="{ close }">
      <Button variant="ghost" @click="close">Cancel</Button>
      <Button
        :disabled="!changed"
        @click="save(close)"
      >
        Save
      </Button>
    </template>
  </Modal>
</template>
