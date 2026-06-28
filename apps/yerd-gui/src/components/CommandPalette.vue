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

// Known groups render first in this order; everything else (the per-site groups)
// follows, sorted by name descending.
const KNOWN_GROUPS = ["Go to", "General", "Actions", "Sites"];

const groups = computed(() => {
  const q = query.value.trim().toLowerCase();
  const items = props.commands.filter(
    (c) => c.inPalette && (!q || c.title.toLowerCase().includes(q)),
  );
  const by = new Map<string, Command[]>();
  for (const c of items) {
    const list = by.get(c.group) ?? [];
    list.push(c);
    by.set(c.group, list);
  }
  const known = KNOWN_GROUPS.filter((g) => by.has(g));
  const sites = [...by.keys()]
    .filter((g) => !KNOWN_GROUPS.includes(g))
    .sort()
    .reverse();
  return [...known, ...sites].map((title) => ({
    title,
    items: by.get(title) ?? [],
  }));
});

const flat = computed(() => groups.value.flatMap((g) => g.items));

watch(
  () => props.open,
  (isOpen) => {
    if (!isOpen) return;
    query.value = "";
    selected.value = 0;
    void nextTick(() => input.value?.focus());
  },
);

watch(flat, () => {
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
  const n = flat.value.length;
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
    choose(flat.value[selected.value]);
  }
}
</script>

<template>
  <Teleport to="body">
    <div
      v-if="open"
      class="fixed inset-0 z-50 flex items-start justify-center p-4 pt-[12vh]"
    >
      <div class="absolute inset-0 bg-black/50 rounded-[10px] animate-fade-in" @click="close" />
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
        <div class="min-h-0 flex-1 overflow-auto p-1">
          <template v-for="group in groups" :key="group.title">
            <p
              class="px-3 pb-1 pt-2 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70"
            >
              {{ group.title }}
            </p>
            <ul>
              <li
                v-for="cmd in group.items"
                :key="cmd.id"
                :class="
                  cmd === flat[selected]
                    ? 'flex cursor-pointer items-center justify-between rounded-md bg-muted px-3 py-2 text-sm'
                    : 'flex cursor-pointer items-center justify-between rounded-md px-3 py-2 text-sm'
                "
                @click="choose(cmd)"
                @mousemove="selected = flat.indexOf(cmd)"
              >
                <span>{{ cmd.title }}</span>
                <span
                  v-if="cmd.chord"
                  class="ml-4 shrink-0 text-xs text-muted-foreground"
                >
                  {{ formatChord(cmd.chord, mac) }}
                </span>
              </li>
            </ul>
          </template>
          <p
            v-if="flat.length === 0"
            class="px-3 py-2 text-sm text-muted-foreground"
          >
            No matching commands
          </p>
        </div>
      </div>
    </div>
  </Teleport>
</template>
