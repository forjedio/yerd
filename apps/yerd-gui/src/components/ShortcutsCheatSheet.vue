<script setup lang="ts">
import { computed } from "vue";

import Modal from "@/components/ui/Modal.vue";
import type { Chord } from "@/lib/shortcuts/chord";
import { formatChord } from "@/lib/shortcuts/format";
import { isMac } from "@/lib/shortcuts/platform";
import { nativeShortcuts, type Command } from "@/lib/shortcuts/registry";

const props = defineProps<{ open: boolean; commands: Command[] }>();
defineEmits<{ "update:open": [boolean] }>();

const mac = isMac();
const GROUP_ORDER = ["Go to", "General", "Actions", "View", "Window"];

const groups = computed(() => {
  const by = new Map<string, { title: string; chord: Chord }[]>();
  const add = (group: string, title: string, chord: Chord): void => {
    const list = by.get(group) ?? [];
    list.push({ title, chord });
    by.set(group, list);
  };
  for (const cmd of props.commands) {
    if (cmd.chord) add(cmd.group, cmd.title, cmd.chord);
  }
  for (const n of nativeShortcuts(mac)) add(n.group, n.title, n.chord);
  return GROUP_ORDER.filter((g) => by.has(g)).map((title) => ({
    title,
    items: by.get(title) ?? [],
  }));
});
</script>

<template>
  <Modal
    title="Keyboard shortcuts"
    size="lg"
    :open="open"
    @update:open="$emit('update:open', $event)"
  >
    <div class="grid grid-cols-1 gap-6 sm:grid-cols-2">
      <section v-for="group in groups" :key="group.title">
        <h3
          class="mb-2 text-[11px] font-semibold uppercase tracking-wider text-muted-foreground/70"
        >
          {{ group.title }}
        </h3>
        <ul class="flex flex-col gap-1">
          <li
            v-for="cmd in group.items"
            :key="cmd.title"
            class="flex items-center justify-between gap-4 text-sm"
          >
            <span>{{ cmd.title }}</span>
            <kbd
              class="shrink-0 rounded border bg-muted px-1.5 py-0.5 text-xs text-muted-foreground"
            >
              {{ formatChord(cmd.chord, mac) }}
            </kbd>
          </li>
        </ul>
      </section>
    </div>
  </Modal>
</template>
