<script setup lang="ts">
import { CircleCheck, RotateCw, ShieldCheck, Square, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";
import { RouterLink } from "vue-router";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { StatusReport } from "@/ipc/types";
import { healthRows, metaSegments, siteHeadline, statusKind } from "@/lib/overview";
import { cn } from "@/lib/utils";

/**
 * The Overview's environment strip: serving state and site-count headline, the
 * daemon details line, the health chips and Restart/Stop. One row when wide,
 * stacked when compact. Display only; the view owns the daemon calls and
 * confirmation dialogs.
 */
const props = defineProps<{
  report: StatusReport;
  siteCount: number;
  /** The view's in-flight lifecycle action (`"daemon"` or `"restart:daemon"`), or null. */
  busy: string | null;
}>();

const emit = defineEmits<{ stop: []; restart: [] }>();

const kind = computed(() => statusKind(props.report, props.siteCount));
const headline = computed(() => siteHeadline(kind.value, props.siteCount));
const meta = computed(() => metaSegments(props.report).join(" · "));
const rows = computed(() => healthRows(props.report));
const LABELS = { serving: "Serving", "not-serving": "Not serving", ready: "Ready" } as const;
const label = computed(() => LABELS[kind.value]);
const bad = computed(() => kind.value === "not-serving");

const CHIP_TONES = {
  ok: "bg-card text-muted-foreground dark:bg-background",
  warn: "border-warning/40 bg-warning/10 text-warning",
  bad: "border-destructive/40 bg-destructive/10 text-destructive",
} as const;

const LINK_TONES = {
  ok: "",
  warn: "bg-warning/20",
  bad: "bg-destructive/20",
} as const;
</script>

<template>
  <section
    data-testid="status-card"
    class="grid grid-cols-[minmax(0,1fr)_auto] gap-x-5 gap-y-3 rounded-xl border bg-muted px-5 py-4 text-card-foreground dark:bg-card xl:grid-cols-[auto_1px_minmax(0,1fr)_auto] xl:items-center"
  >
    <div class="col-start-1 row-start-1 min-w-0">
      <div class="flex items-center gap-2">
        <span class="relative flex size-2" aria-hidden="true">
          <span
            :class="
              cn(
                'absolute inline-flex h-full w-full rounded-full opacity-60 motion-safe:animate-ping',
                bad ? 'bg-destructive' : 'bg-success',
              )
            "
          />
          <span
            :class="cn('relative inline-flex size-2 rounded-full', bad ? 'bg-destructive' : 'bg-success')"
          />
        </span>
        <span
          data-testid="status-label"
          :class="
            cn(
              'text-[11px] font-semibold uppercase tracking-[0.6px]',
              bad ? 'text-destructive' : 'text-success',
            )
          "
        >
          {{ label }}
        </span>
      </div>
      <h2 class="mt-1 whitespace-nowrap text-[22px] font-semibold leading-tight tracking-[-0.3px]">
        {{ headline.text }}
        <span v-if="headline.showTld" class="font-mono text-lg font-medium text-muted-foreground"
          >.{{ report.tld }}</span
        >
      </h2>
    </div>

    <span class="hidden h-11 w-px bg-border xl:col-start-2 xl:row-start-1 xl:block" aria-hidden="true" />

    <div
      class="col-span-2 row-start-2 min-w-0 xl:col-span-1 xl:col-start-3 xl:row-start-1"
    >
      <p class="truncate font-mono text-xs text-faint" :title="meta">{{ meta }}</p>
      <ul v-if="rows.length" data-testid="health-rows" class="mt-2 flex flex-wrap gap-1.5">
        <li
          v-for="row in rows"
          :key="row.key"
          :data-tone="row.tone"
          :class="
            cn(
              'inline-flex h-6 min-w-0 max-w-full items-center gap-1.5 rounded-md border pl-2 text-xs',
              row.link ? 'pr-1' : 'pr-2',
              CHIP_TONES[row.tone],
            )
          "
        >
          <template v-if="row.tone === 'ok'">
            <ShieldCheck v-if="row.key === 'trust'" class="size-3 shrink-0 text-success" aria-hidden="true" />
            <CircleCheck v-else class="size-3 shrink-0 text-success" aria-hidden="true" />
          </template>
          <TriangleAlert v-else class="size-3 shrink-0" aria-hidden="true" />
          <span class="min-w-0 truncate" :title="row.text">{{ row.text }}</span>
          <RouterLink
            v-if="row.link"
            :to="row.link.to"
            :class="
              cn(
                'shrink-0 rounded px-1.5 py-px text-[11px] font-semibold hover:underline',
                LINK_TONES[row.tone],
              )
            "
          >
            {{ row.link.label }}
          </RouterLink>
        </li>
      </ul>
    </div>

    <div class="col-start-2 row-start-1 flex shrink-0 items-start gap-2 xl:col-start-4 xl:items-center">
      <Button variant="outline" size="sm" :disabled="busy !== null" @click="emit('restart')">
        <Spinner v-if="busy === 'restart:daemon'" class="!size-3.5" />
        <RotateCw v-else class="!size-3.5" /> Restart
      </Button>
      <Button variant="outline" size="sm" :disabled="busy !== null" @click="emit('stop')">
        <Spinner v-if="busy === 'daemon'" class="!size-3.5" />
        <Square v-else class="!size-3" /> Stop
      </Button>
    </div>
  </section>
</template>
