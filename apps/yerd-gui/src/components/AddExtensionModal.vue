<script setup lang="ts">
import { ref, watch } from "vue";
import { FolderOpen } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
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

/** Bumped whenever the dialog closes, reopens or changes version, so a reply to
 *  an add from an earlier session can't close or write into the current one. */
let session = 0;

watch([() => props.open, () => props.version], () => {
  session += 1;
  busy.value = false;
  path.value = "";
  name.value = "";
  zend.value = false;
  problem.value = null;
});

async function browse(): Promise<void> {
  const picked = await pickExtensionFile();
  if (picked === null) return;
  path.value = picked;
  problem.value = null;
}

const FIELD =
  "h-10 rounded-md border bg-background px-3 text-[13px] placeholder:text-faint focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";

async function add(): Promise<void> {
  const chosen = path.value.trim();
  if (!chosen) {
    problem.value = "choose an extension file";
    return;
  }
  const mine = session;
  const version = props.version;
  busy.value = true;
  problem.value = null;
  try {
    const map = await addPhpExtension(version, chosen, zend.value, name.value.trim() || undefined);
    toast.success("Extension registered", "Loaded into the FPM pool and CLI.");
    emit("added", map);
    if (mine === session) emit("update:open", false);
  } catch (e) {
    if (mine === session) problem.value = (e as IpcError).message;
    else toast.error(`Couldn't add the extension to PHP ${version}`, (e as IpcError).message);
  } finally {
    if (mine === session) busy.value = false;
  }
}
</script>

<template>
  <DialogFrame
    :open="open"
    :title="`Add an extension to PHP ${version}`"
    width="lg"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <div class="flex flex-col gap-3">
      <div>
        <div class="flex items-center gap-2">
          <input
            id="ext-path"
            v-model="path"
            :class="FIELD"
            class="min-w-0 flex-1 font-mono"
            placeholder="/opt/homebrew/lib/php/pecl/20250925/scrypt.so"
            aria-label="Extension path"
            :aria-invalid="problem ? 'true' : undefined"
          />
          <Button variant="outline" class="h-10 shrink-0" @click="browse">
            <FolderOpen />
            Choose
          </Button>
        </div>
        <p v-if="problem" class="mt-1.5 text-xs text-destructive">{{ problem }}</p>
      </div>

      <input
        id="ext-name"
        v-model="name"
        :class="FIELD"
        placeholder="Name — defaults to the file name"
        aria-label="Extension name"
      />

      <label class="mt-1 flex items-center gap-2.5 text-[13px] text-muted-foreground">
        <Switch v-model="zend" aria-label="Load as a Zend extension" />
        Load as a Zend extension (Xdebug, OPcache)
      </label>
    </div>

    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Cancel</Button>
      <Button :disabled="busy" @click="add">
        <Spinner v-if="busy" class="size-4" />
        {{ busy ? "Checking…" : "Add extension" }}
      </Button>
    </template>
  </DialogFrame>
</template>
