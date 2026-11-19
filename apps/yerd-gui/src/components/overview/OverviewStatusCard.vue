<script setup lang="ts">
import { ArrowRight, CircleCheck, RotateCw, Square, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";
import { RouterLink } from "vue-router";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { StatusReport } from "@/ipc/types";
import { healthRows, metaSegments, siteHeadline, statusKind } from "@/lib/overview";
import { cn } from "@/lib/utils";

/**
 * The Overview's status card: serving state, Restart/Stop, the site-count
 * headline, the daemon meta line and the inset health rows. Display only; the
 * view owns the daemon calls and confirmation dialogs.
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
</script>

<template>
  <section
    data-testid="status-card"
    class="rounded-xl border bg-card px-5 pb-5 pt-[18px] text-card-foreground"
  >
    <div class="flex h-[30px] items-center justify-between gap-3">
      <div class="flex items-center gap-2">
        <span class="relative flex size-2.5" aria-hidden="true">
          <span
            :class="
              cn(
                'absolute inline-flex h-full w-full rounded-full opacity-60 motion-safe:animate-ping',
                bad ? 'bg-destructive' : 'bg-success',
              )
            "
          />
          <span
            :class="cn('relative inline-flex size-2.5 rounded-full', bad ? 'bg-destructive' : 'bg-success')"
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
      <div class="flex shrink-0 gap-2">
        <Button variant="outline" size="sm" :disabled="busy !== null" @click="emit('restart')">
          <Spinner v-if="busy === 'restart:daemon'" class="!size-3.5" />
          <RotateCw v-else class="!size-3.5" /> Restart
        </Button>
        <Button variant="outline" size="sm" :disabled="busy !== null" @click="emit('stop')">
          <Spinner v-if="busy === 'daemon'" class="!size-3.5" />
          <Square v-else class="!size-3" /> Stop
        </Button>
      </div>
    </div>

    <h2 class="mt-3 text-[26px] font-semibold leading-tight tracking-[-0.4px]">
      {{ headline.text }}
      <span v-if="headline.showTld" class="font-mono text-xl font-medium text-muted-foreground"
        >.{{ report.tld }}</span
      >
    </h2>

    <p class="mt-2 truncate font-mono text-xs text-faint" :title="meta">{{ meta }}</p>

    <ul
      v-if="rows.length"
      data-testid="health-rows"
      class="mt-3 overflow-hidden rounded-lg border bg-muted text-[13px] dark:bg-background"
    >
      <li
        v-for="(row, i) in rows"
        :key="row.key"
        :data-tone="row.tone"
        :class="
          cn(
            'flex min-h-[30px] items-center gap-2 py-1.5 pl-3 pr-3',
            i > 0 && 'border-t',
            row.tone === 'warn' && 'bg-warning/10 text-warning',
            row.tone === 'bad' && 'bg-destructive/10 text-destructive',
            row.tone === 'ok' && 'text-muted-foreground',
          )
        "
      >
        <CircleCheck
          v-if="row.tone === 'ok'"
          class="size-[13px] shrink-0 text-success"
          aria-hidden="true"
        />
        <TriangleAlert v-else class="size-[13px] shrink-0" aria-hidden="true" />
        <span class="min-w-0 flex-1 truncate" :title="row.text">{{ row.text }}</span>
        <RouterLink
          v-if="row.link"
          :to="row.link.to"
          class="inline-flex shrink-0 items-center gap-1 text-xs font-semibold hover:underline"
        >
          {{ row.link.label }} <ArrowRight class="size-3" aria-hidden="true" />
        </RouterLink>
      </li>
    </ul>
  </section>
</template>
