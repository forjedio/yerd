<script setup lang="ts">
import { computed } from "vue";
import { FolderOpen } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import Switch from "@/components/ui/Switch.vue";
import { pickDirectory } from "@/ipc/client";

const props = defineProps<{
  /** Prefix for the field ids (`${idPrefix}-name`, `-location`, `-php`). */
  idPrefix: string;
  parkedFolders: string[];
  phpOptions: { value: string; label: string }[];
  domain: string;
  projectPath: string;
  nameValid: boolean;
  /** Shown in place of the PHP picker when no usable version is installed. */
  noPhpText: string;
}>();

const name = defineModel<string>("name", { required: true });
const location = defineModel<string>("location", { required: true });
const php = defineModel<string>("php", { required: true });
const secure = defineModel<boolean>("secure", { required: true });

/** Parked roots, plus a custom-picked folder that isn't one of them. */
const locationOptions = computed(() => {
  const opts = props.parkedFolders.map((f) => ({ value: f, label: `${f}  (parked)` }));
  if (location.value && !props.parkedFolders.includes(location.value)) {
    opts.unshift({ value: location.value, label: location.value });
  }
  return opts;
});

async function chooseLocation(): Promise<void> {
  const dir = await pickDirectory(location.value || undefined);
  if (dir) location.value = dir;
}
</script>

<template>
  <div class="space-y-4">
    <div>
      <label class="text-sm font-medium" :for="`${idPrefix}-name`">Project name</label>
      <Input :id="`${idPrefix}-name`" v-model="name" placeholder="e.g. blog" class="mt-2" />
      <p class="mt-1 text-xs text-muted-foreground">
        Served at
        <span class="font-mono text-foreground">{{ domain }}</span>
        <span v-if="projectPath"> · creates <span class="font-mono">{{ projectPath }}</span></span>
      </p>
      <p v-if="name && !nameValid" class="mt-1 text-xs text-destructive">
        Use a single label: letters, numbers and hyphens only.
      </p>
    </div>

    <div>
      <label class="text-sm font-medium" :for="`${idPrefix}-location`">Location</label>
      <div class="mt-2 flex gap-2">
        <Select
          v-if="locationOptions.length"
          :id="`${idPrefix}-location`"
          :model-value="location"
          :options="locationOptions"
          class="w-full"
          aria-label="Location"
          @update:model-value="(v: string) => (location = v)"
        />
        <Input v-else :model-value="location" readonly placeholder="Choose a folder…" />
        <Button variant="outline" @click="chooseLocation">
          <FolderOpen class="size-4" /> Browse
        </Button>
      </div>
      <p class="mt-1 text-xs text-muted-foreground">
        A parked folder serves the new site automatically; any other folder is linked.
      </p>
    </div>

    <div class="flex items-center justify-between gap-4 rounded-lg border p-3">
      <div>
        <p class="text-sm font-medium">PHP version</p>
        <p class="text-xs text-muted-foreground">
          <slot name="php-hint">The version this site runs on.</slot>
        </p>
      </div>
      <Select
        v-if="phpOptions.length"
        :id="`${idPrefix}-php`"
        :model-value="php"
        :options="phpOptions"
        class="w-40 shrink-0"
        aria-label="PHP version"
        @update:model-value="(v: string) => (php = v)"
      />
      <span v-else class="shrink-0 text-xs text-destructive">{{ noPhpText }}</span>
    </div>

    <div class="flex items-center justify-between gap-4 rounded-lg border p-3">
      <div>
        <p class="text-sm font-medium">HTTPS</p>
        <p class="text-xs text-muted-foreground">Serve this site over TLS.</p>
      </div>
      <Switch v-model="secure" aria-label="Serve over HTTPS" />
    </div>
  </div>
</template>
