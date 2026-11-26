<script setup lang="ts">
import { ArrowUpRight, ChevronDown, Plus, Sparkles, Wrench } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import { useResource } from "@/composables/useResource";
import { openInBrowser, releaseNotes } from "@/ipc/client";
import type { ReleaseNotesEntry } from "@/ipc/types";
import {
  cappedList,
  FEATURES_SHOWN,
  FIXES_SHOWN,
  formatReleaseDate,
  releaseList,
  releaseStats,
  releaseSummary,
} from "@/lib/overview";
import { cn } from "@/lib/utils";

/**
 * The Overview's "What's new" card, from the daemon's cached `release_notes`:
 * the selected release on the left (summary line, plus its NEW and FIXED lists
 * and stats when expanded) and the releases on the right. Collapsed, the right
 * pane shows only the selected release, or every release when none was
 * published for the running version; expanding only adds height. Fetches
 * whenever the daemon comes up; while it is down the card keeps whatever this
 * session already loaded.
 */
const props = defineProps<{
  /** The daemon connection state (`null` before the first probe). */
  running: boolean | null;
  /** The running daemon's version, shown when no notes are published for it. */
  installedVersion?: string;
}>();

const RELEASES_URL = "https://github.com/forjedio/yerd/releases";

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

const skeleton = computed(() => !data.value && (props.running === null || loading.value));
const current = computed(() => data.value?.current ?? null);
const releases = computed(() => (data.value ? releaseList(data.value) : []));
const selectedTag = ref<string | null>(null);
const expanded = ref(false);

const selected = computed<ReleaseNotesEntry | null>(
  () => releases.value.find((r) => r.tag === selectedTag.value) ?? current.value,
);
const summary = computed(() => (selected.value ? releaseSummary(selected.value) : null));
const features = computed(() => cappedList(selected.value?.features ?? [], FEATURES_SHOWN));
const fixes = computed(() => cappedList(selected.value?.fixes ?? [], FIXES_SHOWN));
const stats = computed(() => (selected.value ? releaseStats(selected.value).join(" · ") : ""));
const listed = computed(() =>
  expanded.value || !current.value || !selected.value ? releases.value : [selected.value],
);

function isInstalled(entry: ReleaseNotesEntry | null): boolean {
  return (
    !!entry &&
    entry === current.value &&
    (!props.installedVersion || props.installedVersion === entry.version)
  );
}

function shortDate(iso: string): string {
  return formatReleaseDate(iso).replace(/ \d{4}$/, "");
}

function openNotes(): void {
  const tag = selected.value?.tag;
  void openInBrowser(tag ? `${RELEASES_URL}/tag/${encodeURIComponent(tag)}` : RELEASES_URL);
}
</script>

<template>
  <div
    v-if="skeleton"
    data-testid="release-notes-skeleton"
    class="space-y-3 rounded-xl border bg-muted p-5 dark:bg-card"
  >
    <div class="h-5 w-40 animate-pulse rounded bg-card dark:bg-background" />
    <div class="h-3 w-2/3 animate-pulse rounded bg-card dark:bg-background" />
  </div>

  <section
    v-else-if="data"
    data-testid="release-notes"
    class="grid grid-cols-[minmax(0,1fr)_220px] overflow-hidden rounded-xl border bg-muted text-card-foreground dark:bg-card xl:grid-cols-[minmax(0,1fr)_320px]"
  >
    <div data-testid="release-current" class="flex min-w-0 flex-col px-6 py-5">
      <div class="flex min-w-0 items-center gap-2.5">
        <Sparkles class="size-4 shrink-0 text-brand" aria-hidden="true" />
        <span class="truncate font-mono text-lg font-semibold">
          {{ selected?.tag ?? (installedVersion ? `v${installedVersion}` : "This version") }}
        </span>
        <span
          v-if="isInstalled(selected) || !selected"
          class="inline-flex shrink-0 items-center gap-1.5 rounded-full bg-success/15 px-2 py-0.5 text-[11px] font-semibold text-success"
        >
          <span class="size-1.5 rounded-full bg-success" aria-hidden="true" /> Installed
        </span>
        <span
          v-if="selected?.prerelease"
          class="shrink-0 rounded-full border px-2 py-0.5 text-[11px] font-medium text-muted-foreground"
        >
          Pre-release
        </span>
        <span v-if="selected" class="hidden shrink-0 text-xs text-faint xl:inline">
          {{ formatReleaseDate(selected.published_at) }}
        </span>
        <button
          type="button"
          data-testid="release-link"
          class="ml-auto inline-flex shrink-0 items-center gap-1 rounded-md text-[13px] font-medium text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          aria-label="Full release notes"
          @click="openNotes"
        >
          <span class="hidden xl:inline">Full release notes</span>
          <ArrowUpRight class="size-3.5" aria-hidden="true" />
        </button>
        <button
          v-if="selected"
          type="button"
          data-testid="release-toggle"
          class="flex size-7 shrink-0 items-center justify-center rounded-md border text-muted-foreground transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          :aria-expanded="expanded"
          :aria-label="expanded ? 'Collapse release notes' : 'Expand release notes'"
          @click="expanded = !expanded"
        >
          <ChevronDown
            :class="cn('size-3.5 transition-transform', expanded && 'rotate-180')"
            aria-hidden="true"
          />
        </button>
      </div>

      <p v-if="summary" data-testid="release-summary" class="mt-3 text-[13px] text-muted-foreground">
        {{ summary.lead }}<span v-if="summary.more" class="text-faint">… {{ summary.more }}</span>
      </p>
      <p v-else class="mt-3 text-[13px] text-muted-foreground">
        No release notes published for this version.
      </p>

      <template v-if="expanded && selected">
        <div class="mt-5 grid gap-6 xl:grid-cols-2">
          <div v-if="selected.features.length" data-testid="release-new">
            <span
              class="inline-flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-[0.6px] text-brand"
            >
              <Plus class="size-3" aria-hidden="true" /> New
            </span>
            <ul class="mt-2 space-y-1.5 text-[13px]">
              <li v-for="(it, i) in features.shown" :key="i" class="flex gap-2">
                <span class="mt-[7px] size-1 shrink-0 rounded-full bg-faint" aria-hidden="true" />
                <span class="min-w-0">{{ it.title }}</span>
              </li>
              <li v-if="features.more > 0" class="flex gap-2 text-faint">
                <span class="mt-[7px] size-1 shrink-0 rounded-full bg-faint" aria-hidden="true" />
                + {{ features.more }} more {{ features.more === 1 ? "feature" : "features" }}
              </li>
            </ul>
          </div>
          <div v-if="selected.fixes.length" data-testid="release-fixed">
            <span
              class="inline-flex items-center gap-1.5 text-[11px] font-semibold uppercase tracking-[0.6px] text-success"
            >
              <Wrench class="size-3" aria-hidden="true" /> Fixed
            </span>
            <ul class="mt-2 space-y-1.5 text-[13px] text-muted-foreground">
              <li v-for="(it, i) in fixes.shown" :key="i" class="flex gap-2">
                <span class="mt-[7px] size-1 shrink-0 rounded-full bg-faint" aria-hidden="true" />
                <span class="min-w-0">{{ it.title }}</span>
              </li>
              <li v-if="fixes.more > 0" class="flex gap-2 text-faint">
                <span class="mt-[7px] size-1 shrink-0 rounded-full bg-faint" aria-hidden="true" />
                + {{ fixes.more }} more {{ fixes.more === 1 ? "fix" : "fixes" }}
              </li>
            </ul>
          </div>
        </div>
        <p data-testid="release-stats" class="mt-auto pt-6 font-mono text-[11px] text-faint">
          {{ stats }}
        </p>
      </template>
    </div>

    <div class="min-w-0 border-l bg-sidebar px-3 py-4 dark:bg-background">
      <p class="px-1 text-[11px] font-semibold uppercase tracking-[0.8px] text-faint">Releases</p>
      <ul class="mt-2.5 space-y-1">
        <li v-for="rel in listed" :key="rel.tag">
          <button
            type="button"
            data-testid="release-row"
            :aria-pressed="rel === selected"
            :class="
              cn(
                'w-full rounded-lg border px-3 py-2 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
                rel === selected
                  ? 'border-brand/40 bg-selected'
                  : 'border-transparent hover:bg-accent',
              )
            "
            @click="selectedTag = rel.tag"
          >
            <span class="flex items-center gap-2">
              <span class="truncate font-mono text-[13px] font-semibold">{{ rel.tag }}</span>
              <span
                v-if="isInstalled(rel)"
                class="size-1.5 shrink-0 rounded-full bg-success"
                aria-label="Installed"
              />
              <span class="ml-auto shrink-0 text-[11px] text-faint">{{ shortDate(rel.published_at) }}</span>
            </span>
            <span
              :class="
                cn(
                  'mt-0.5 block truncate text-xs',
                  rel === selected ? 'text-muted-foreground' : 'text-faint',
                )
              "
              :title="rel.summary"
            >
              {{ rel.summary }}
            </span>
          </button>
        </li>
      </ul>
    </div>
  </section>

  <p
    v-else-if="running === false"
    class="rounded-xl border bg-muted px-5 py-4 text-[13px] text-muted-foreground dark:bg-card"
  >
    Release notes appear once Yerd is running.
  </p>
  <p
    v-else-if="error"
    class="rounded-xl border bg-muted px-5 py-4 text-[13px] text-muted-foreground dark:bg-card"
  >
    Couldn't load release notes.
  </p>
</template>
