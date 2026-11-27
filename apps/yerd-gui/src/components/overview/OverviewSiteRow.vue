<script setup lang="ts">
import { ArrowUpRight, Copy, Lock, LockOpen, Pencil, Star } from "lucide-vue-next";
import { computed } from "vue";

import { openInBrowser } from "@/ipc/client";
import type { SiteEntry, StatusReport } from "@/ipc/types";
import { tildify } from "@/lib/sites";
import { openTitle, siteHost, siteUrl } from "@/lib/siteUrl";
import { copyWithToast, toastOnError } from "@/lib/toastActions";
import { cn } from "@/lib/utils";

/**
 * One site on the Overview's Pinned and Recently added lists: its star, domain
 * and path, the Shared/WordPress badges, padlock and PHP pill, then edit and
 * open. `toggle-star` and `edit` are handled by the card.
 */
const props = defineProps<{
  site: SiteEntry;
  report: StatusReport;
  home: string | null;
  shared: boolean;
  busy: boolean;
}>();

const emit = defineEmits<{ "toggle-star": []; edit: [] }>();

const host = computed(() => siteHost(props.site, props.report.tld));
const path = computed(() => tildify(props.site.document_root, props.home));
const starred = computed(() => props.site.favourite === true);

async function copyPath(): Promise<void> {
  await copyWithToast(props.site.document_root, { title: "Path copied" });
}

async function open(): Promise<void> {
  await toastOnError("Couldn't open site", () => openInBrowser(siteUrl(props.site, props.report)));
}

const ICON_BUTTON =
  "flex size-7 shrink-0 items-center justify-center rounded-md text-faint transition-colors hover:bg-accent hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
</script>

<template>
  <div data-testid="favourite-row" class="flex h-[52px] items-center gap-2 pl-1.5 pr-2">
    <button
      type="button"
      :class="cn(ICON_BUTTON, starred ? 'text-warning hover:text-warning' : '', 'disabled:opacity-50')"
      :disabled="busy"
      :aria-label="`${starred ? 'Unfavourite' : 'Favourite'} ${host}`"
      :title="starred ? 'Remove from favourites' : 'Add to favourites'"
      data-testid="favourite-star"
      @click="emit('toggle-star')"
    >
      <Star :class="cn('size-3.5', starred && 'fill-current')" aria-hidden="true" />
    </button>

    <div class="min-w-0 flex-1">
      <p class="truncate font-mono text-[13px] font-medium" data-testid="site-domain">{{ host }}</p>
      <div class="flex min-w-0 items-center gap-1.5">
        <span class="truncate font-mono text-[11px] text-faint" :title="site.document_root">{{ path }}</span>
        <button
          type="button"
          class="shrink-0 rounded text-faint transition-colors hover:text-foreground focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
          :aria-label="`Copy path of ${host}`"
          title="Copy path"
          data-testid="copy-path"
          @click="copyPath"
        >
          <Copy class="size-3" aria-hidden="true" />
        </button>
      </div>
    </div>

    <span
      v-if="shared"
      data-testid="shared-badge"
      class="inline-flex h-5 shrink-0 items-center gap-1 rounded bg-success/15 px-1.5 text-[11px] font-medium text-success"
    >
      <span class="size-1.5 rounded-full bg-success" aria-hidden="true" /> Shared
    </span>
    <span
      v-if="site.is_wordpress"
      data-testid="wordpress-badge"
      class="inline-flex h-5 shrink-0 items-center rounded border border-brand/60 px-1.5 text-[11px] font-medium text-brand"
    >
      WordPress
    </span>
    <Lock v-if="site.secure" class="size-3.5 shrink-0 text-success" aria-label="Secured" />
    <LockOpen v-else class="size-3.5 shrink-0 text-faint" aria-label="Not secured" />
    <span
      data-testid="favourite-php"
      :class="
        cn(
          'inline-flex h-5 min-w-9 shrink-0 items-center justify-center rounded bg-secondary px-1.5 font-mono text-[11px] font-semibold',
          site.php !== report.default_php ? 'text-warning' : 'text-foreground',
        )
      "
    >
      {{ site.php }}
    </span>
    <span class="mx-0.5 h-4 w-px shrink-0 bg-border" aria-hidden="true" />
    <button
      type="button"
      :class="ICON_BUTTON"
      :aria-label="`Edit ${host}`"
      title="Edit in Sites"
      data-testid="site-edit"
      @click="emit('edit')"
    >
      <Pencil class="size-3.5" aria-hidden="true" />
    </button>
    <button
      type="button"
      :class="cn(ICON_BUTTON, '-ml-1.5')"
      :aria-label="`Open ${host}`"
      :title="openTitle(site, report)"
      data-testid="site-open"
      @click="open"
    >
      <ArrowUpRight class="size-3.5" aria-hidden="true" />
    </button>
  </div>
</template>
