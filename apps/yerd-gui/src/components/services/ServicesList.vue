<script setup lang="ts">
import { ChevronRight } from "lucide-vue-next";

import ServiceIcon from "@/components/services/ServiceIcon.vue";
import { stateDotClass } from "@/components/services/styles";
import Spinner from "@/components/ui/Spinner.vue";
import type { ServiceRunState, ServiceStatus } from "@/ipc/types";
import {
  detailsCell,
  hasVersion,
  type ServiceSection,
  siteDomain,
  statusLine,
  statusWord,
  versionLabel,
} from "@/lib/services";
import { cn } from "@/lib/utils";

/**
 * The Services list, grouped Databases / Cache & search / Per-site. `list` is
 * the column beside the inspector (wide windows): an icon tile, the name and
 * version (or a Reverb's site) and a status line with the database count.
 * `table` spans the window (compact windows) with VERSION, PORT, STATUS and
 * DETAILS columns; a click opens the inspector drawer and there is no
 * persistent selection.
 */
const props = defineProps<{
  layout: "list" | "table";
  sections: ServiceSection[];
  selected: string | null;
  /** Database counts of running SQL engines, by service id. */
  dbCounts: Record<string, number>;
  tld: string;
  /** The view's in-flight action key, e.g. `restart:postgres`. */
  busy: string | null;
}>();

const emit = defineEmits<{ select: [id: string] }>();

function count(s: ServiceStatus): number | null {
  return props.dbCounts[s.service] ?? null;
}

function rowBusy(s: ServiceStatus): boolean {
  return props.busy?.endsWith(`:${s.service}`) ?? false;
}

function statusClass(state: ServiceRunState): string {
  if (state === "running") return "text-muted-foreground";
  if (state === "failed") return "text-destructive";
  return "text-faint";
}

const LABEL = "px-2 pb-1.5 pt-4 text-[11px] font-semibold uppercase tracking-[0.6px] text-faint";
const TABLE_COLS =
  "grid grid-cols-[minmax(0,1fr)_100px_90px_130px_170px_20px] items-center gap-x-2";
</script>

<template>
  <div class="flex min-h-0 flex-col" data-testid="services-list" :data-layout="layout">
    <div class="min-h-0 flex-1 overflow-y-auto pb-3 pr-4">
      <div
        v-if="layout === 'table'"
        :class="cn(TABLE_COLS, 'px-2.5 pb-1 pt-3 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
        aria-hidden="true"
      >
        <span>Service</span><span>Version</span><span>Port</span><span>Status</span><span>Details</span><span />
      </div>

      <section
        v-for="sec in sections"
        :key="sec.group"
        role="group"
        :aria-label="sec.label"
        :data-section="sec.group"
      >
        <template v-if="layout === 'list'">
          <p :class="cn(LABEL, sec === sections[0] && 'pt-0')">{{ sec.label }}</p>
          <div class="flex flex-col gap-1">
            <button
              v-for="s in sec.items"
              :key="s.service"
              type="button"
              :data-key="s.service"
              :aria-current="selected === s.service ? 'true' : undefined"
              :class="
                cn(
                  'flex h-[58px] w-full items-center gap-3 rounded-lg border px-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
                  selected === s.service
                    ? 'border-brand/45 bg-brand/[0.14]'
                    : 'border-transparent hover:bg-accent/60',
                )
              "
              @click="emit('select', s.service)"
            >
              <span
                :class="
                  cn(
                    'flex size-8 shrink-0 items-center justify-center rounded-md border',
                    selected === s.service ? 'border-brand/30 bg-brand/15 text-brand' : 'bg-card text-muted-foreground',
                  )
                "
              >
                <ServiceIcon :type-id="s.type_id ?? s.service" class="size-4" />
              </span>
              <span class="min-w-0 flex-1">
                <span class="flex items-baseline gap-2">
                  <span
                    :class="
                      cn(
                        'truncate text-[13px] font-semibold',
                        s.state === 'stopped' && 'text-muted-foreground',
                      )
                    "
                  >
                    {{ s.display_name }}
                  </span>
                  <span v-if="hasVersion(s)" class="shrink-0 font-mono text-xs text-faint">{{ versionLabel(s) }}</span>
                  <span v-else-if="s.site" class="truncate font-mono text-xs text-faint">{{ siteDomain(s, tld) }}</span>
                </span>
                <span class="mt-1 flex items-center gap-1.5 text-xs" data-testid="row-status">
                  <span :class="cn('size-1.5 shrink-0 rounded-full', stateDotClass(s.state))" aria-hidden="true" />
                  <span :class="cn('truncate', statusClass(s.state))">{{ statusLine(s, count(s)) }}</span>
                </span>
              </span>
              <Spinner v-if="rowBusy(s)" class="size-3.5 shrink-0" />
            </button>
          </div>
        </template>

        <template v-else>
          <p class="flex h-[30px] items-end px-2.5 pb-1.5 text-[11px] font-medium uppercase tracking-[0.6px] text-muted-foreground">
            {{ sec.label }}
          </p>
          <button
            v-for="s in sec.items"
            :key="s.service"
            type="button"
            :data-key="s.service"
            :aria-label="s.site ? `${s.display_name} ${siteDomain(s, tld)}` : s.display_name"
            :class="
              cn(
                TABLE_COLS,
                'h-12 w-full rounded-lg px-2.5 text-left transition-colors hover:bg-accent/60 focus-visible:bg-brand/25 focus-visible:outline-none',
              )
            "
            @click="emit('select', s.service)"
          >
            <span class="flex min-w-0 items-center gap-2">
              <span class="min-w-0">
                <span class="block truncate text-[13px] font-semibold">{{ s.display_name }}</span>
                <span v-if="s.site" class="block truncate font-mono text-[11px] text-faint">{{ siteDomain(s, tld) }}</span>
              </span>
              <Spinner v-if="rowBusy(s)" class="size-3.5 shrink-0" />
            </span>
            <span class="truncate font-mono text-xs text-muted-foreground" data-testid="cell-version">
              {{ hasVersion(s) ? versionLabel(s) : "—" }}
            </span>
            <span class="truncate font-mono text-xs text-muted-foreground">:{{ s.port }}</span>
            <span class="flex items-center gap-1.5 text-xs" data-testid="cell-status">
              <span :class="cn('size-1.5 shrink-0 rounded-full', stateDotClass(s.state))" aria-hidden="true" />
              <span :class="statusClass(s.state)">{{ statusWord(s.state) }}</span>
            </span>
            <span class="truncate text-xs text-faint" data-testid="cell-details">{{ detailsCell(s, count(s)) }}</span>
            <ChevronRight class="size-4 text-faint" aria-hidden="true" />
          </button>
        </template>
      </section>
    </div>
  </div>
</template>
