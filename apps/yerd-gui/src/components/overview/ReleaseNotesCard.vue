<script setup lang="ts">
import { ChevronDown, Sparkles, Tag, Wrench } from "lucide-vue-next";
import type { Component } from "vue";
import { computed, ref, watch } from "vue";

import { useResource } from "@/composables/useResource";
import { releaseNotes } from "@/ipc/client";
import type { ReleaseNoteItem, ReleaseNotesEntry } from "@/ipc/types";
import { formatReleaseDate } from "@/lib/overview";
import { cn } from "@/lib/utils";

/**
 * The running release's categorised notes and the two releases before it, from
 * the daemon's cached `release_notes`. Fetches whenever the daemon comes up;
 * while it is down the card keeps whatever this session already loaded.
 */
const props = defineProps<{
  /** The daemon connection state (`null` before the first probe). */
  running: boolean | null;
  /** The running daemon's version, shown when no notes are published for it. */
  installedVersion?: string;
}>();

const EARLIER_SHOWN = 2;

const { data, loading, error, refresh } = useResource("release-notes", releaseNotes, {
  immediate: false,
});

watch(
  () => props.running,
  (up) => {
    if (up) void refresh();
  },
  { immediate: true },
);

interface Group {
  key: string;
  label: string;
  icon: Component;
  chip: string;
  item: string;
  items: ReleaseNoteItem[];
}

function groupsOf(entry: ReleaseNotesEntry): Group[] {
  const all: Group[] = [
    {
      key: "new",
      label: "New",
      icon: Sparkles,
      chip: "bg-brand/10 text-brand",
      item: "text-foreground",
      items: entry.features,
    },
    {
      key: "fixed",
      label: "Fixed",
      icon: Wrench,
      chip: "bg-success/10 text-success",
      item: "text-muted-foreground",
      items: entry.fixes,
    },
    {
      key: "other",
      label: "Other",
      icon: Tag,
      chip: "bg-secondary text-muted-foreground",
      item: "text-muted-foreground",
      items: entry.others,
    },
  ];
  return all.filter((g) => g.items.length > 0);
}

const skeleton = computed(() => !data.value && (props.running === null || loading.value));
const current = computed(() => data.value?.current ?? null);
const earlier = computed(() => (data.value?.earlier ?? []).slice(0, EARLIER_SHOWN));
const expanded = ref(new Set<string>());

function toggle(tag: string): void {
  const next = new Set(expanded.value);
  if (next.has(tag)) next.delete(tag);
  else next.add(tag);
  expanded.value = next;
}
</script>

<template>
  <div v-if="skeleton" data-testid="release-notes-skeleton" class="flex flex-col gap-3.5">
    <div class="space-y-3 rounded-xl border bg-card p-5">
      <div class="h-5 w-40 animate-pulse rounded bg-muted" />
      <div class="h-3 w-3/4 animate-pulse rounded bg-muted" />
      <div class="h-3 w-2/3 animate-pulse rounded bg-muted" />
      <div class="h-3 w-1/2 animate-pulse rounded bg-muted" />
    </div>
    <div class="space-y-3 rounded-xl border bg-card p-5">
      <div class="h-3 w-2/3 animate-pulse rounded bg-muted" />
      <div class="h-3 w-1/2 animate-pulse rounded bg-muted" />
    </div>
  </div>

  <div v-else-if="data" class="flex flex-col gap-3.5">
    <section
      data-testid="release-current"
      class="rounded-xl border border-brand/40 bg-card px-5 pb-5 pt-[18px] text-card-foreground"
    >
      <div class="flex flex-wrap items-center gap-x-3 gap-y-1.5">
        <span class="font-mono text-lg font-semibold">
          {{ current?.tag ?? (installedVersion ? `v${installedVersion}` : "This version") }}
        </span>
        <span
          class="inline-flex items-center gap-1.5 rounded-full bg-success/10 px-2 py-0.5 text-[11px] font-semibold text-success"
        >
          <span class="size-1.5 rounded-full bg-success" aria-hidden="true" /> Installed
        </span>
        <span
          v-if="current?.prerelease"
          class="rounded-full border px-2 py-0.5 text-[11px] font-medium text-muted-foreground"
        >
          Pre-release
        </span>
        <span v-if="current" class="ml-auto text-xs text-faint">
          {{ formatReleaseDate(current.published_at) }}
        </span>
      </div>

      <template v-if="current">
        <div v-for="g in groupsOf(current)" :key="g.key" class="mt-4">
          <span
            :class="
              cn(
                'inline-flex h-5 items-center gap-1 rounded px-1.5 text-[11px] font-semibold uppercase tracking-[0.4px]',
                g.chip,
              )
            "
          >
            <component :is="g.icon" class="size-3" aria-hidden="true" /> {{ g.label }}
          </span>
          <ul :class="cn('mt-2 list-disc space-y-2 pl-4 text-[13px]', g.item)">
            <li v-for="(it, i) in g.items" :key="i">{{ it.title }}</li>
          </ul>
        </div>
      </template>
      <p v-else class="mt-3 text-[13px] text-muted-foreground">
        No release notes published for this version.
      </p>
    </section>

    <section
      v-if="earlier.length"
      data-testid="release-earlier"
      class="overflow-hidden rounded-xl border bg-card text-card-foreground"
    >
      <div v-for="(rel, i) in earlier" :key="rel.tag" :class="cn(i > 0 && 'border-t')">
        <button
          type="button"
          data-testid="release-earlier-row"
          class="flex min-h-14 w-full items-center gap-4 px-5 py-2.5 text-left transition-colors hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring"
          :aria-expanded="expanded.has(rel.tag)"
          @click="toggle(rel.tag)"
        >
          <span class="w-[88px] shrink-0">
            <span class="block font-mono text-[13px] font-semibold">{{ rel.tag }}</span>
            <span class="block text-[11px] text-faint">{{ formatReleaseDate(rel.published_at) }}</span>
          </span>
          <span class="min-w-0 flex-1 truncate text-[13px] text-muted-foreground" :title="rel.summary">
            {{ rel.summary }}
          </span>
          <ChevronDown
            :class="
              cn(
                'size-[15px] shrink-0 text-faint transition-transform',
                expanded.has(rel.tag) && 'rotate-180',
              )
            "
            aria-hidden="true"
          />
        </button>
        <div v-if="expanded.has(rel.tag)" class="px-5 pb-4">
          <div v-for="g in groupsOf(rel)" :key="g.key" class="mt-3 first:mt-0">
            <span
              :class="
                cn(
                  'inline-flex h-5 items-center gap-1 rounded px-1.5 text-[11px] font-semibold uppercase tracking-[0.4px]',
                  g.chip,
                )
              "
            >
              <component :is="g.icon" class="size-3" aria-hidden="true" /> {{ g.label }}
            </span>
            <ul :class="cn('mt-2 list-disc space-y-2 pl-4 text-[13px]', g.item)">
              <li v-for="(it, j) in g.items" :key="j">{{ it.title }}</li>
            </ul>
          </div>
        </div>
      </div>
    </section>
  </div>

  <p
    v-else-if="running === false"
    class="rounded-xl border bg-card px-5 py-4 text-[13px] text-muted-foreground"
  >
    Release notes appear once Yerd is running.
  </p>
  <p v-else-if="error" class="rounded-xl border bg-card px-5 py-4 text-[13px] text-muted-foreground">
    Couldn't load release notes.
  </p>
</template>
