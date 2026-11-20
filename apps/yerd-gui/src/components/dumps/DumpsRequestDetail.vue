<script setup lang="ts">
import { Search, X } from "lucide-vue-next";
import { computed, nextTick, ref } from "vue";

import DumpsEventCard from "@/components/dumps/DumpsEventCard.vue";
import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import { TooltipProvider } from "@/components/ui/tooltip";
import {
  CATEGORY_CHIPS,
  type ChipKey,
  chipLabel,
  type DumpRequest,
  matchesQuery,
  metaLine,
  requestCards,
  requestCategoryCounts,
  requestTitle,
  statusTone,
  TAG,
} from "@/lib/dumps";
import { CHIP_TONE, cn } from "@/lib/utils";

/**
 * One request: method, status and path (or the site when the request summary
 * is missing), the meta line, the category chips with per-request counts, the
 * event filter and the event cards. Shared by the Dumps screen (panel or
 * drawer) and the Dumps window. `openFilter()` expands the filter and focuses it.
 */
const props = defineProps<{
  request: DumpRequest | null;
  /** Show a close button (drawer mode). */
  closable?: boolean;
}>();

const category = defineModel<ChipKey>("category", { required: true });
const query = defineModel<string>("query", { required: true });

const emit = defineEmits<{ close: [] }>();

const filterOpen = ref(false);
const filterInput = ref<InstanceType<typeof Input> | null>(null);
const filterShown = computed(() => filterOpen.value || query.value !== "");

const counts = computed(() => (props.request ? requestCategoryCounts(props.request) : null));
const cards = computed(() => {
  if (!props.request) return [];
  return requestCards(props.request).filter(
    (e) => (category.value === "all" || e.category === category.value) && matchesQuery(e, query.value),
  );
});

const emptyTitle = computed(() => {
  if (query.value.trim()) return `No events match “${query.value.trim()}”`;
  if (category.value === "all") return "Only the request summary was captured";
  return `No ${chipLabel(category.value).toLowerCase()} in this request`;
});

async function openFilter(): Promise<void> {
  filterOpen.value = true;
  await nextTick();
  filterInput.value?.focus();
}

function closeFilter(): void {
  query.value = "";
  filterOpen.value = false;
}

function onEmptyAction(): void {
  if (query.value.trim()) closeFilter();
  else category.value = "all";
}

function onFilterBlur(): void {
  if (query.value === "") filterOpen.value = false;
}

defineExpose({ openFilter });

const CHIP =
  "inline-flex h-[26px] items-center gap-1.5 rounded-full px-2.5 text-[13px] font-medium transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
</script>

<template>
  <div class="flex min-h-0 flex-col px-7 pb-6 pt-[22px]" data-testid="dumps-detail-body">
    <template v-if="request">
      <div class="flex shrink-0 items-start gap-3">
        <div class="min-w-0 flex-1">
          <div class="flex min-w-0 items-center gap-2">
            <template v-if="request.summary">
              <span v-if="request.summary.method" :class="cn(TAG, CHIP_TONE.brand)">{{ request.summary.method }}</span>
              <span
                v-if="request.summary.status"
                :class="cn(TAG, CHIP_TONE[statusTone(request.summary.status)])"
              >
                {{ request.summary.status }}
              </span>
            </template>
            <h2
              class="min-w-0 truncate font-mono text-[19px] font-semibold tracking-[-0.2px]"
              :title="requestTitle(request)"
              data-testid="detail-title"
            >
              {{ requestTitle(request) }}
            </h2>
          </div>
          <p class="mt-1.5 text-xs text-muted-foreground" data-testid="detail-meta">
            <template v-if="!request.summary">No request summary · </template>{{ metaLine(request) }}
          </p>
        </div>
        <Button
          v-if="closable"
          variant="outline"
          size="icon"
          class="size-8 shrink-0"
          aria-label="Close"
          title="Close"
          @click="emit('close')"
        >
          <X />
        </Button>
      </div>

      <div class="mt-4 flex shrink-0 items-start gap-3">
        <div class="flex min-w-0 flex-1 flex-wrap items-center gap-1" role="radiogroup" aria-label="Event category">
          <button
            v-for="chip in CATEGORY_CHIPS"
            :key="chip.key"
            type="button"
            role="radio"
            :aria-checked="category === chip.key"
            :data-chip="chip.key"
            :class="
              cn(
                CHIP,
                category === chip.key ? 'bg-brand font-semibold text-white' : 'text-muted-foreground hover:text-foreground',
              )
            "
            @click="category = chip.key"
          >
            {{ chip.label }}
            <span :class="cn('text-[11px]', category === chip.key ? 'text-white/70' : 'text-faint')">
              {{ counts?.[chip.key] ?? 0 }}
            </span>
          </button>
        </div>
        <div v-if="filterShown" class="relative w-[220px] shrink-0">
          <Search
            class="pointer-events-none absolute left-2.5 top-1/2 size-3.5 -translate-y-1/2 text-muted-foreground"
            aria-hidden="true"
          />
          <Input
            ref="filterInput"
            v-model="query"
            class="h-[30px] pl-8 pr-2.5 text-[13px]"
            placeholder="Filter events…"
            aria-label="Filter events"
            data-testid="event-filter"
            @keydown.esc.stop.prevent="closeFilter"
            @blur="onFilterBlur"
          />
        </div>
        <Button
          v-else
          variant="outline"
          size="icon"
          class="size-[30px] shrink-0"
          aria-label="Filter events"
          title="Filter events"
          @click="openFilter"
        >
          <Search class="!size-3.5" />
        </Button>
      </div>

      <TooltipProvider :delay-duration="300">
        <div v-if="cards.length > 0" class="mt-3 flex flex-col gap-2">
          <DumpsEventCard v-for="e in cards" :key="e.id" :event="e" />
        </div>
        <div
          v-else
          class="mt-3 flex flex-col items-center rounded-[10px] border px-6 py-8 text-center"
          data-testid="detail-empty"
        >
          <p class="text-[15px] font-semibold">{{ emptyTitle }}</p>
          <button
            v-if="query.trim() || category !== 'all'"
            type="button"
            class="mt-1.5 text-[13px] font-semibold text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
            @click="onEmptyAction"
          >
            {{ query.trim() ? "Clear filter" : "Show all events" }}
          </button>
        </div>
      </TooltipProvider>
    </template>

    <p v-else class="flex flex-1 items-center justify-center text-[13px] text-faint">
      Select a request to see its events.
    </p>
  </div>
</template>
