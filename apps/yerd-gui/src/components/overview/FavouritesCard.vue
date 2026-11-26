<script setup lang="ts">
import { ArrowRight, Layers, Star } from "lucide-vue-next";
import { computed, ref } from "vue";
import { RouterLink, useRouter } from "vue-router";

import OverviewSiteRow from "@/components/overview/OverviewSiteRow.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import { setSiteFavourite } from "@/ipc/client";
import type { GroupsState, SiteEntry, StatusReport } from "@/ipc/types";
import { pinnedView, recentlyAdded } from "@/lib/overview";
import { sitesSelectIntent } from "@/lib/shortcuts/sitesIntent";
import { toastOnError } from "@/lib/toastActions";

/**
 * The Overview's site card: starred sites under their groups (Pinned) or the
 * most recently added sites, toggled in the header. A row's star toggles the
 * favourite and its pencil opens the site on the Sites page. `changed` asks
 * the view to reload the sites.
 */
const props = defineProps<{
  sites: SiteEntry[];
  report: StatusReport;
  totalSites: number;
  groups: GroupsState;
  home: string | null;
  shared: Set<string>;
}>();

const emit = defineEmits<{ changed: [] }>();

const router = useRouter();
const tab = ref<"pinned" | "recent">("pinned");

const pinned = computed(() => pinnedView(props.sites, props.groups));
const starredCount = computed(() => props.sites.filter((s) => s.favourite === true).length);
const recent = computed(() => recentlyAdded(props.sites));
const options = computed(() => [
  { value: "pinned", label: `Pinned ${starredCount.value}` },
  { value: "recent", label: "Recently added" },
]);

function onTab(value: string): void {
  if (value === "pinned" || value === "recent") tab.value = value;
}

const toggling = ref<string | null>(null);

async function toggleStar(site: SiteEntry): Promise<void> {
  if (toggling.value) return;
  toggling.value = site.name;
  try {
    await toastOnError("Couldn't change favourite", () =>
      setSiteFavourite(site.name, site.favourite !== true),
    );
    emit("changed");
  } finally {
    toggling.value = null;
  }
}

function edit(site: SiteEntry): void {
  sitesSelectIntent.value = site.name;
  void router.push("/sites");
}
</script>

<template>
  <section
    data-testid="favourites-card"
    class="flex min-w-0 flex-col rounded-xl border bg-muted px-5 pb-3 pt-4 text-card-foreground dark:bg-card"
  >
    <div class="flex items-center gap-3 pb-3">
      <SegmentedControl
        :model-value="tab"
        :options="options"
        variant="plain"
        size="md"
        aria-label="Sites to show"
        @update:model-value="onTab"
      />
      <RouterLink
        to="/sites"
        class="ml-auto inline-flex shrink-0 items-center gap-1 text-[13px] font-medium text-brand hover:underline"
      >
        All {{ totalSites }} sites <ArrowRight class="size-3.5" aria-hidden="true" />
      </RouterLink>
    </div>

    <template v-if="tab === 'pinned'">
      <div
        v-if="pinned.groups.length === 0 && pinned.ungrouped.length === 0"
        data-testid="favourites-empty"
        class="flex flex-col items-center justify-center rounded-lg border border-dashed px-6 py-12 text-center"
      >
        <span class="flex size-9 items-center justify-center rounded-full border bg-card">
          <Star class="size-4 text-muted-foreground" aria-hidden="true" />
        </span>
        <p class="mt-3 text-sm font-semibold">No favourites yet</p>
        <p class="mt-1 max-w-xs text-xs text-faint">
          Star a site on the Sites page and it'll show up here.
        </p>
      </div>

      <template v-else>
        <div
          v-for="g in pinned.groups"
          :key="g.name"
          data-testid="pinned-group"
          class="mb-2 overflow-hidden rounded-lg border bg-card dark:bg-background"
        >
          <div class="flex h-9 items-center gap-2 border-b px-3">
            <Layers class="size-3.5 shrink-0 text-brand" aria-hidden="true" />
            <span class="truncate text-[13px] font-semibold">{{ g.name }}</span>
            <span class="shrink-0 text-xs text-faint">
              {{ g.starred.length }}/{{ g.total }} sites starred
            </span>
          </div>
          <ul class="divide-y">
            <li v-for="s in g.starred" :key="s.name">
              <OverviewSiteRow
                :site="s"
                :report="report"
                :home="home"
                :shared="shared.has(s.name)"
                :busy="toggling !== null"
                @toggle-star="toggleStar(s)"
                @edit="edit(s)"
              />
            </li>
          </ul>
        </div>
        <ul v-if="pinned.ungrouped.length" data-testid="pinned-ungrouped" class="divide-y">
          <li v-for="s in pinned.ungrouped" :key="s.name">
            <OverviewSiteRow
              :site="s"
              :report="report"
              :home="home"
              :shared="shared.has(s.name)"
              :busy="toggling !== null"
              @toggle-star="toggleStar(s)"
              @edit="edit(s)"
            />
          </li>
        </ul>
      </template>
    </template>

    <template v-else>
      <p
        v-if="recent.length === 0"
        data-testid="recent-empty"
        class="rounded-lg border border-dashed px-6 py-10 text-center text-xs text-faint"
      >
        No recently added sites yet. Sites you link, create or park from now on appear here.
      </p>
      <ul v-else data-testid="recent-list" class="divide-y">
        <li v-for="s in recent" :key="s.name">
          <OverviewSiteRow
            :site="s"
            :report="report"
            :home="home"
            :shared="shared.has(s.name)"
            :busy="toggling !== null"
            @toggle-star="toggleStar(s)"
            @edit="edit(s)"
          />
        </li>
      </ul>
    </template>
  </section>
</template>
