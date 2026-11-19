<script setup lang="ts">
import {
  Download,
  ExternalLink,
  FolderOpen,
  Link,
  RefreshCw,
  XCircle,
} from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { ToolStatus } from "@/ipc/types";
import { toolDescription } from "@/lib/tooling";

/**
 * One tool in the Tooling screen's Available column: idle (Install, or Install
 * Yerd's copy over an external one), blocked on Composer, installing, or
 * failed. The view owns the install; this card only renders its state and
 * emits intents.
 */
defineProps<{
  tool: ToolStatus;
  state: "idle" | "installing" | "failed";
  /** Needs Yerd's own Composer before it can install. */
  blocked: boolean;
  /** Another mutation is running, so every action waits. */
  locked: boolean;
  /** The latest streamed log line while installing. */
  detail?: string;
  error?: string;
}>();

const emit = defineEmits<{
  install: [];
  installComposer: [];
  showLog: [];
  reveal: [string];
}>();

const CHIP = "inline-flex h-[18px] shrink-0 items-center gap-1 rounded px-1.5 text-[11px] font-medium";
const CMD =
  "inline-flex h-[18px] items-center rounded bg-background px-1.5 font-mono text-[11px] text-muted-foreground";
</script>

<template>
  <div
    :class="[
      'rounded-[10px] border p-4',
      state === 'installing'
        ? 'border-brand/40 bg-brand/5'
        : state === 'failed'
          ? 'border-destructive/40 bg-destructive/5'
          : 'bg-card',
    ]"
    :data-testid="`tool-card-${tool.id}`"
    :data-state="state"
  >
    <div class="flex flex-wrap items-center gap-2">
      <span class="text-sm font-semibold">{{ tool.display_name }}</span>
      <span v-if="state === 'failed'" :class="[CHIP, 'bg-destructive/15 text-destructive']">
        <XCircle class="size-3" aria-hidden="true" /> Install failed
      </span>
      <span v-else-if="state === 'installing'" :class="[CHIP, 'bg-brand/15 text-brand']">
        <Spinner class="size-3 text-brand" /> Installing
      </span>
      <span v-else-if="tool.external" :class="[CHIP, 'bg-warning/15 text-warning']">
        <ExternalLink class="size-3" aria-hidden="true" /> External
      </span>
    </div>

    <template v-if="state === 'failed'">
      <p class="mt-1.5 break-words text-xs text-destructive" data-testid="tool-card-error">
        {{ error }}
      </p>
      <div class="mt-4 grid grid-cols-2 gap-2">
        <Button variant="outline" size="sm" @click="emit('showLog')">Show log</Button>
        <Button size="sm" :disabled="locked" @click="emit('install')">
          <RefreshCw /> Try again
        </Button>
      </div>
    </template>

    <template v-else>
      <p class="mt-1.5 text-xs leading-relaxed text-muted-foreground">
        {{ toolDescription(tool.id) }}
      </p>
      <div v-if="tool.binaries.length" class="mt-2 flex flex-wrap gap-1.5">
        <code v-for="b in tool.binaries" :key="b" :class="CMD">{{ b }}</code>
      </div>

      <template v-if="state === 'installing'">
        <div
          class="mt-4 h-1.5 overflow-hidden rounded-full bg-muted"
          role="progressbar"
          :aria-label="`Installing ${tool.display_name}`"
        >
          <div class="h-full w-1/3 animate-pulse rounded-full bg-brand/60" />
        </div>
        <div class="mt-2 flex items-center gap-2 text-xs">
          <span class="min-w-0 flex-1 truncate text-muted-foreground" data-testid="tool-card-detail">
            {{ detail || "Starting…" }}
          </span>
          <button
            type="button"
            class="shrink-0 font-medium text-brand hover:underline"
            @click="emit('showLog')"
          >
            Show log
          </button>
        </div>
      </template>

      <template v-else-if="tool.external">
        <div
          v-if="tool.external_path"
          class="mt-3 flex items-center gap-2 rounded-md border bg-background px-2.5 py-2"
        >
          <div class="min-w-0 flex-1">
            <div class="text-[10px] font-medium uppercase tracking-[0.6px] text-faint">
              Location
            </div>
            <div
              class="truncate font-mono text-xs"
              :title="tool.external_path"
              data-testid="tool-card-location"
            >
              {{ tool.external_path }}
            </div>
          </div>
          <Button
            variant="ghost"
            size="icon"
            class="size-7 shrink-0 text-muted-foreground"
            :aria-label="`Reveal ${tool.external_path}`"
            @click="emit('reveal', tool.external_path)"
          >
            <FolderOpen class="!size-3.5" />
          </Button>
        </div>
        <p class="mt-2.5 text-[11px] leading-relaxed text-faint" data-testid="tool-card-note">
          This is the {{ tool.binaries[0] ?? tool.display_name }} your terminal runs now. Yerd's
          copy would go ahead of it on your PATH.
        </p>
      </template>

      <template v-if="state === 'idle'">
        <div
          v-if="blocked"
          class="mt-3 flex items-center gap-1.5 rounded-md bg-warning/15 px-3 py-2 text-xs text-warning"
          data-testid="tool-card-needs-composer"
        >
          <Link class="size-3.5 shrink-0" aria-hidden="true" />
          <span>Needs Composer —</span>
          <button
            type="button"
            class="font-medium text-brand hover:underline disabled:pointer-events-none disabled:opacity-50"
            :disabled="locked"
            @click="emit('installComposer')"
          >
            install it first
          </button>
        </div>
        <Button
          v-if="tool.external"
          variant="outline"
          size="sm"
          class="mt-3 w-full border-brand/40 text-brand hover:text-brand"
          :disabled="locked || blocked"
          @click="emit('install')"
        >
          <Download /> Install Yerd's copy
        </Button>
        <Button
          v-else
          size="sm"
          class="mt-3 w-full"
          :disabled="locked || blocked"
          @click="emit('install')"
        >
          <Download /> Install
        </Button>
      </template>
    </template>
  </div>
</template>
