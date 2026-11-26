<script setup lang="ts">
import { ChevronDown } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import { useResource } from "@/composables/useResource";
import { useWideLayout } from "@/composables/useWideLayout";
import { contributors, openInBrowser } from "@/ipc/client";
import { contributorTiles } from "@/lib/overview";
import { cn } from "@/lib/utils";

/**
 * The people behind Yerd, from the daemon's cached `contributors` (GitHub): the
 * top contributors plus a tile counting the rest that opens the full graph.
 * Collapsible, expanded on every visit; while the daemon is down it keeps
 * whatever this session already loaded.
 */
const props = defineProps<{
  /** The daemon connection state (`null` before the first probe). */
  running: boolean | null;
}>();

const GRAPH_URL = "https://github.com/forjedio/yerd/graphs/contributors";

const { data, loading, error, refresh } = useResource("contributors", contributors, {
  immediate: false,
});

watch(
  () => props.running,
  (up) => {
    if (up) void refresh();
  },
  { immediate: true },
);

const wide = useWideLayout();
const expanded = ref(true);
const skeleton = computed(() => !data.value && (props.running === null || loading.value));
const total = computed(() => data.value?.total ?? 0);
const tiles = computed(() =>
  contributorTiles(data.value?.contributors ?? [], total.value, wide.value ? 3 : 5),
);
const failed = ref(new Set<string>());

function profileUrl(login: string): string {
  return `https://github.com/${encodeURIComponent(login)}`;
}

function onAvatarError(login: string): void {
  failed.value = new Set(failed.value).add(login);
}

const TILE =
  "flex h-10 w-full min-w-0 items-center gap-2.5 rounded-lg border bg-card px-2.5 text-left transition-colors hover:border-brand/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring dark:bg-background";
</script>

<template>
  <section
    data-testid="contributors-card"
    class="rounded-xl border bg-muted px-5 pb-4 pt-4 text-card-foreground dark:bg-card"
  >
    <div class="flex items-center gap-3">
      <h3 class="text-[15px] font-semibold">Contributors</h3>
      <span v-if="data" class="text-xs text-faint">
        {{ total }} {{ total === 1 ? "person" : "people" }} · by commits
      </span>
      <button
        type="button"
        data-testid="contributors-toggle"
        class="ml-auto flex size-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        :aria-expanded="expanded"
        :aria-label="expanded ? 'Collapse contributors' : 'Expand contributors'"
        @click="expanded = !expanded"
      >
        <ChevronDown
          :class="cn('size-3.5 transition-transform', expanded && 'rotate-180')"
          aria-hidden="true"
        />
      </button>
    </div>

    <template v-if="expanded">
      <div
        v-if="skeleton"
        data-testid="contributors-skeleton"
        class="mt-3 grid grid-cols-3 gap-2 xl:grid-cols-2"
      >
        <div v-for="i in 4" :key="i" class="h-10 animate-pulse rounded-lg bg-card dark:bg-background" />
      </div>

      <template v-else-if="data">
        <ul
          v-if="tiles.shown.length"
          data-testid="contributors-grid"
          class="mt-3 grid grid-cols-3 gap-2 xl:grid-cols-2"
        >
          <li v-for="c in tiles.shown" :key="c.login" class="min-w-0">
            <button
              type="button"
              data-testid="contributor"
              :title="`Open ${c.login} on GitHub`"
              :class="TILE"
              @click="openInBrowser(profileUrl(c.login))"
            >
              <img
                v-if="!failed.has(c.login)"
                :src="c.avatar_url"
                alt=""
                loading="lazy"
                referrerpolicy="no-referrer"
                class="size-[22px] shrink-0 rounded-full"
                @error="onAvatarError(c.login)"
              />
              <span
                v-else
                data-testid="contributor-initial"
                class="flex size-[22px] shrink-0 items-center justify-center rounded-full bg-secondary text-[11px] font-semibold text-secondary-foreground"
                aria-hidden="true"
              >
                {{ c.login.charAt(0).toUpperCase() }}
              </span>
              <span class="min-w-0">
                <span class="block truncate font-mono text-xs font-semibold leading-tight">
                  {{ c.login }}
                </span>
                <span class="block truncate text-[11px] leading-tight text-faint">
                  {{ c.commits }} {{ c.commits === 1 ? "commit" : "commits" }}
                </span>
              </span>
            </button>
          </li>
          <li v-if="tiles.more > 0" class="min-w-0">
            <button
              type="button"
              data-testid="contributors-more"
              class="flex h-10 w-full items-center justify-center rounded-lg border border-dashed px-2.5 text-xs font-medium text-muted-foreground transition-colors hover:border-brand/50 hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
              @click="openInBrowser(GRAPH_URL)"
            >
              +{{ tiles.more }} more {{ tiles.more === 1 ? "contributor" : "contributors" }}
            </button>
          </li>
        </ul>
        <p v-else class="mt-3 text-[13px] text-muted-foreground">No contributors listed.</p>
      </template>

      <p v-else-if="running === false" class="mt-3 text-[13px] text-muted-foreground">
        Contributors appear once Yerd is running.
      </p>
      <p v-else-if="error" class="mt-3 text-[13px] text-muted-foreground">
        Couldn't load contributors.
      </p>
    </template>
  </section>
</template>
