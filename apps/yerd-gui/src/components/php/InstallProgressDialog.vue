<script setup lang="ts">
import { computed } from "vue";
import { CheckCircle2, LoaderCircle, TriangleAlert } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import type { PhpVersion } from "@/ipc/types";
import { parseInstallLine } from "@/lib/php";

/**
 * A PHP install's progress. Closing it never stops the install: Yerd keeps going
 * in the background, the sidebar shows the operation and a toast reports the
 * outcome. `detail` is the latest streamed job line; a per-file download line
 * drives the bar, anything else shows as text over an indeterminate bar.
 */
const props = defineProps<{
  open: boolean;
  version: PhpVersion;
  phase: "running" | "done" | "error";
  detail: string;
  error: string;
}>();

const emit = defineEmits<{ "update:open": [boolean] }>();

const progress = computed(() => parseInstallLine(props.detail));

const line = computed(() => {
  const p = progress.value;
  if (!p) return props.detail || "Preparing the download…";
  return p.totalMb
    ? `Downloading ${p.file} — ${p.doneMb} of ${p.totalMb} MB`
    : `Downloading ${p.file} — ${p.doneMb} MB`;
});

const title = computed(() => {
  if (props.phase === "done") return `PHP ${props.version} installed`;
  if (props.phase === "error") return `Couldn't install PHP ${props.version}`;
  return `Installing PHP ${props.version}`;
});
</script>

<template>
  <DialogFrame
    :open="open"
    :title="title"
    width="md"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <template #title>
      <LoaderCircle v-if="phase === 'running'" class="size-4 shrink-0 animate-spin text-brand" />
      <CheckCircle2 v-else-if="phase === 'done'" class="size-4 shrink-0 text-success" />
      <TriangleAlert v-else class="size-4 shrink-0 text-destructive" />
      <span class="truncate">{{ title }}</span>
    </template>

    <div v-if="phase === 'running'" data-testid="install-progress">
      <div
        class="h-1.5 overflow-hidden rounded-full bg-muted"
        role="progressbar"
        :aria-valuenow="progress?.percent ?? undefined"
        aria-valuemin="0"
        aria-valuemax="100"
        :aria-label="`Installing PHP ${version}`"
      >
        <div
          v-if="progress?.percent != null"
          class="h-full rounded-full bg-brand transition-[width]"
          :style="{ width: `${progress.percent}%` }"
          data-testid="install-bar"
        />
        <div v-else class="h-full w-1/3 animate-pulse rounded-full bg-brand/60" data-testid="install-bar-indeterminate" />
      </div>
      <div class="mt-2.5 flex items-center gap-3 text-xs">
        <span class="min-w-0 flex-1 truncate text-muted-foreground" data-testid="install-line">{{ line }}</span>
        <span v-if="progress?.percent != null" class="shrink-0 font-mono text-faint">{{ progress.percent }}%</span>
      </div>
      <p class="mt-3 text-xs text-faint">
        You can close this — Yerd keeps installing and tells you when it's done.
      </p>
    </div>
    <p v-else-if="phase === 'done'" class="text-sm text-muted-foreground">
      PHP {{ version }} is ready to use.
    </p>
    <p v-else class="text-sm text-muted-foreground">{{ error }}</p>

    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Close</Button>
    </template>
  </DialogFrame>
</template>
