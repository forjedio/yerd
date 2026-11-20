<script setup lang="ts">
import { ArrowRight, ArrowUpRight, Lock, LockOpen, Star } from "lucide-vue-next";
import { computed, ref } from "vue";
import { RouterLink } from "vue-router";

import { openInBrowser, setSiteFavourite } from "@/ipc/client";
import type { SiteEntry, StatusReport } from "@/ipc/types";
import { favouriteSites, siteSubtitle } from "@/lib/overview";
import { openTitle, siteHost, siteUrl } from "@/lib/siteUrl";
import { toastOnError } from "@/lib/toastActions";
import { cn } from "@/lib/utils";

/**
 * The Overview's starred sites: one row per favourite that opens the site in
 * the browser, with its star to unfavourite it, or the draft's dashed empty
 * state when nothing is starred. `changed` asks the view to reload the sites.
 */
const props = defineProps<{
  sites: SiteEntry[];
  report: StatusReport;
  totalSites: number;
}>();

const emit = defineEmits<{ changed: [] }>();

const favourites = computed(() => favouriteSites(props.sites));
const unstarring = ref<string | null>(null);

async function unfavourite(site: SiteEntry): Promise<void> {
  if (unstarring.value) return;
  unstarring.value = site.name;
  try {
    await toastOnError("Couldn't change favourite", () => setSiteFavourite(site.name, false));
    emit("changed");
  } finally {
    unstarring.value = null;
  }
}

async function open(site: SiteEntry): Promise<void> {
  await toastOnError("Couldn't open site", () => openInBrowser(siteUrl(site, props.report)));
}
</script>

<template>
  <section
    v-if="favourites.length"
    data-testid="favourites-card"
    class="rounded-xl border bg-card px-5 pb-2 pt-[18px] text-card-foreground"
  >
    <div class="flex items-center gap-3 pb-3">
      <h3 class="text-[15px] font-semibold">Favourites</h3>
      <span class="text-xs text-faint">{{ favourites.length }} starred</span>
      <RouterLink
        to="/sites"
        class="ml-auto inline-flex items-center gap-1 text-[13px] font-medium text-brand hover:underline"
      >
        All {{ totalSites }} sites <ArrowRight class="size-3.5" aria-hidden="true" />
      </RouterLink>
    </div>
    <ul>
      <li v-for="s in favourites" :key="s.name" class="border-t">
        <div class="-mx-2 flex items-center rounded-md transition-colors hover:bg-accent/50">
        <button
          type="button"
          class="ml-1 flex size-7 shrink-0 items-center justify-center rounded-md text-warning transition-colors hover:bg-accent focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50"
          :disabled="unstarring !== null"
          :aria-label="`Unfavourite ${siteHost(s, report.tld)}`"
          title="Remove from favourites"
          data-testid="favourite-star"
          @click="unfavourite(s)"
        >
          <Star class="size-3.5 fill-current" aria-hidden="true" />
        </button>
        <button
          type="button"
          data-testid="favourite-row"
          class="flex h-[54px] min-w-0 flex-1 items-center gap-3 rounded-md pl-1.5 pr-2 text-left focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          :title="openTitle(s, report)"
          @click="open(s)"
        >
          <span class="min-w-0 flex-1">
            <span class="block truncate font-mono text-[13px] font-medium">
              {{ siteHost(s, report.tld) }}
            </span>
            <span class="block truncate text-xs text-faint" :title="siteSubtitle(s)">
              {{ siteSubtitle(s) }}
            </span>
          </span>
          <Lock v-if="s.secure" class="size-3 shrink-0 text-success" aria-label="Secured" />
          <LockOpen v-else class="size-3 shrink-0 text-faint" aria-label="Not secured" />
          <span
            data-testid="favourite-php"
            :class="
              cn(
                'w-7 shrink-0 text-right font-mono text-xs',
                s.php !== report.default_php ? 'text-warning' : 'text-muted-foreground',
              )
            "
          >
            {{ s.php }}
          </span>
          <ArrowUpRight class="size-3.5 shrink-0 text-faint" aria-hidden="true" />
        </button>
        </div>
      </li>
    </ul>
  </section>

  <section
    v-else
    data-testid="favourites-empty"
    class="flex flex-col items-center justify-center rounded-xl border border-dashed px-6 py-12 text-center"
  >
    <span class="flex size-9 items-center justify-center rounded-full border bg-card">
      <Star class="size-4 text-muted-foreground" aria-hidden="true" />
    </span>
    <p class="mt-3 text-sm font-semibold">No favourites yet</p>
    <p class="mt-1 max-w-xs text-xs text-faint">
      Star a site on the Sites page and it'll show up here.
    </p>
  </section>
</template>
