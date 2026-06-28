<script setup lang="ts">
import { computed, nextTick, ref, watch } from "vue";

import { formatChord } from "@/lib/shortcuts/format";
import { isMac } from "@/lib/shortcuts/platform";
import type { Command } from "@/lib/shortcuts/registry";

const props = defineProps<{
  open: boolean;
  commands: Command[];
  run: (cmd: Command) => void;
}>();
const emit = defineEmits<{ "update:open": [boolean] }>();

const query = ref("");
const selected = ref(0);
const input = ref<HTMLInputElement | null>(null);
const mac = isMac();

const listed = computed(() => props.commands.filter((c) => c.inPalette));

const filtered = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return listed.value;
  return listed.value.filter((c) => c.title.toLowerCase().includes(q));
});

watch(
  () => props.open,
  (isOpen) => {
    if (!isOpen) return;
    query.value = "";
    selected.value = 0;
    void nextTick(() => input.value?.focus());
  },
);

watch(filtered, () => {
  selected.value = 0;
});

function close(): void {
  emit("update:open", false);
}

function choose(cmd: Command | undefined): void {
  if (!cmd) return;
  close();
  props.run(cmd);
}

function move(delta: number): void {
  const n = filtered.value.length;
  if (n === 0) return;
  selected.value = (selected.value + delta + n) % n;
}

function onKey(e: KeyboardEvent): void {
  if (e.key === "Escape") {
    e.preventDefault();
    close();
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    move(1);
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    move(-1);
  } else if (e.key === "Enter") {
    e.preventDefault();
    choose(filtered.value[selected.value]);
  }
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open"
      class="fixed inset-0 z-50 flex items-start justify-center p-4 pt-[12vh]"
    >
      <div class="absolute inset-0 bg-black/50 animate-fade-in" @click="close" />
      <div
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        class="relative z-10 flex max-h-[70vh] w-full max-w-lg flex-col overflow-hidden rounded-lg border bg-background shadow-lg animate-fade-in"
      >
        <input
          ref="input"
          v-model="query"
          type="text"
          placeholder="Type a command or page…"
          class="w-full shrink-0 border-b bg-transparent px-4 py-3 text-sm outline-none placeholder:text-muted-foreground"
          @keydown="onKey"
        />
        <ul class="min-h-0 flex-1 overflow-auto p-1">
          <li
            v-for="(cmd, i) in filtered"
            :key="cmd.id"
            :class="
              i === selected
                ? 'flex cursor-pointer items-center justify-between rounded-md bg-muted px-3 py-2 text-sm'
                : 'flex cursor-pointer items-center justify-between rounded-md px-3 py-2 text-sm'
            "
            @click="choose(cmd)"
            @mousemove="selected = i"
          >
            <span>{{ cmd.title }}</span>
            <span class="ml-4 shrink-0 text-xs text-muted-foreground">
              {{ formatChord(cmd.chord, mac) }}
            </span>
          </li>
          <li
            v-if="filtered.length === 0"
            class="px-3 py-2 text-sm text-muted-foreground"
          >
            No matching commands
          </li>
        </ul>
      </div>
    </div>
  </Teleport>
</template>
