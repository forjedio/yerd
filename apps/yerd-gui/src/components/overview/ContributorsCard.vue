<script setup lang="ts">
import { ArrowUpRight } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import { useResource } from "@/composables/useResource";
import { contributors, openInBrowser } from "@/ipc/client";

/**
 * The people behind Yerd, from the daemon's cached `contributors` (GitHub). A
 * grid of the top contributors plus links out to the repository; while the
 * daemon is down it keeps whatever this session already loaded.
 */
const props = defineProps<{
  /** The daemon connection state (`null` before the first probe). */
  running: boolean | null;
}>();

const SHOWN = 9;
const REPO_URL = "https://github.com/forjedio/yerd";
const GRAPH_URL = `${REPO_URL}/graphs/contributors`;

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

const skeleton = computed(() => !data.value && (props.running === null || loading.value));
const shown = computed(() => (data.value?.contributors ?? []).slice(0, SHOWN));
const total = computed(() => data.value?.total ?? 0);
const failed = ref(new Set<string>());

function profileUrl(login: string): string {
  return `https://github.com/${encodeURIComponent(login)}`;
}

function onAvatarError(login: string): void {
  failed.value = new Set(failed.value).add(login);
}
</script>

<template>
  <section
    data-testid="contributors-card"
    class="rounded-xl border bg-card px-5 pb-5 pt-[18px] text-card-foreground"
  >
    <div class="flex items-center gap-3">
      <h3 class="text-[15px] font-semibold">Contributors</h3>
      <span v-if="data" class="text-xs text-faint">
        {{ total }} {{ total === 1 ? "person" : "people" }} · by commits
      </span>
      <button
        type="button"
        class="ml-auto inline-flex items-center gap-1 rounded-md text-[13px] font-medium text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        @click="openInBrowser(REPO_URL)"
      >
        GitHub <ArrowUpRight class="size-3.5" aria-hidden="true" />
      </button>
    </div>

    <div
      v-if="skeleton"
      data-testid="contributors-skeleton"
      class="mt-4 grid grid-cols-4 gap-2.5 xl:grid-cols-3"
    >
      <div v-for="i in 6" :key="i" class="h-14 animate-pulse rounded-lg bg-muted" />
    </div>

    <template v-else-if="data">
      <ul v-if="shown.length" class="mt-4 grid grid-cols-4 gap-2.5 xl:grid-cols-3">
        <li v-for="c in shown" :key="c.login" class="min-w-0">
          <button
            type="button"
            data-testid="contributor"
            :title="`Open ${c.login} on GitHub`"
            class="flex h-14 w-full min-w-0 items-center gap-2.5 rounded-lg border bg-muted px-3 text-left transition-colors hover:border-brand/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring dark:bg-background"
            @click="openInBrowser(profileUrl(c.login))"
          >
            <img
              v-if="!failed.has(c.login)"
              :src="c.avatar_url"
              alt=""
              loading="lazy"
              referrerpolicy="no-referrer"
              class="size-7 shrink-0 rounded-full"
              @error="onAvatarError(c.login)"
            />
            <span
              v-else
              data-testid="contributor-initial"
              class="flex size-7 shrink-0 items-center justify-center rounded-full bg-secondary text-xs font-semibold text-secondary-foreground"
              aria-hidden="true"
            >
              {{ c.login.charAt(0).toUpperCase() }}
            </span>
            <span class="min-w-0">
              <span class="block truncate font-mono text-xs font-medium">
                {{ c.login }}
              </span>
              <span class="block truncate text-[11px] text-faint">
                {{ c.commits }} {{ c.commits === 1 ? "commit" : "commits" }}
              </span>
            </span>
          </button>
        </li>
      </ul>
      <p v-else class="mt-3 text-[13px] text-muted-foreground">No contributors listed.</p>

      <Button variant="outline" size="sm" class="mt-3.5 w-full" @click="openInBrowser(GRAPH_URL)">
        More contributors <ArrowUpRight class="!size-3.5" aria-hidden="true" />
      </Button>
    </template>

    <p v-else-if="running === false" class="mt-3 text-[13px] text-muted-foreground">
      Contributors appear once Yerd is running.
    </p>
    <p v-else-if="error" class="mt-3 text-[13px] text-muted-foreground">
      Couldn't load contributors.
    </p>
  </section>
</template>
