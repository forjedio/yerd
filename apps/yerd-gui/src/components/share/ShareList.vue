<script setup lang="ts">
import { ChevronRight, Globe, KeyRound, Link } from "lucide-vue-next";

import { LIST_LABEL } from "@/components/ui/styles";
import { type ShareItem, type ShareSection, stateDotClass, stateLabel, typeFacts } from "@/lib/share";
import { cn } from "@/lib/utils";

/**
 * The Share list, grouped Quick links / Your domain. `list` is the column
 * beside the inspector (wide windows): an icon tile, the site and a status dot
 * with the public host. `table` spans the window (compact windows) with TYPE,
 * PUBLIC URL and STATUS columns; a click opens the inspector drawer and there
 * is no persistent selection.
 */
defineProps<{
  layout: "list" | "table";
  sections: ShareSection[];
  selected: string | null;
}>();

const emit = defineEmits<{ select: [key: string] }>();

function hostText(item: ShareItem): string {
  return item.host ?? "Waiting for URL…";
}

const TABLE_COLS = "grid grid-cols-[minmax(0,1fr)_120px_minmax(0,1.6fr)_140px_20px] items-center gap-x-3";
</script>

<template>
  <div class="flex min-h-0 flex-col" data-testid="share-list" :data-layout="layout">
    <div class="min-h-0 flex-1 overflow-y-auto pb-3 pr-4">
      <div
        v-if="layout === 'table'"
        :class="cn(TABLE_COLS, 'px-2.5 pb-1 pt-3 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
        aria-hidden="true"
      >
        <span>Site</span><span>Type</span><span>Public URL</span><span>Status</span><span />
      </div>

      <section
        v-for="sec in sections"
        :key="sec.kind"
        role="group"
        :aria-label="sec.label"
        :data-section="sec.kind"
      >
        <template v-if="layout === 'list'">
          <p :class="cn(LIST_LABEL, sec === sections[0] && 'pt-0')">{{ sec.label }}</p>
          <div class="flex flex-col gap-1">
            <button
              v-for="item in sec.items"
              :key="item.key"
              type="button"
              :data-key="item.key"
              :aria-current="selected === item.key ? 'true' : undefined"
              :class="
                cn(
                  'flex h-[58px] w-full items-center gap-3 rounded-lg border px-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
                  selected === item.key ? 'border-brand/45 bg-brand/[0.14]' : 'border-transparent hover:bg-accent/60',
                )
              "
              @click="emit('select', item.key)"
            >
              <span
                :class="
                  cn(
                    'flex size-8 shrink-0 items-center justify-center rounded-md border',
                    selected === item.key ? 'border-brand/30 bg-brand/15 text-brand' : 'bg-card text-muted-foreground',
                  )
                "
              >
                <Link v-if="item.kind === 'quick'" class="size-4" aria-hidden="true" />
                <Globe v-else class="size-4" aria-hidden="true" />
              </span>
              <span class="min-w-0 flex-1">
                <span class="flex items-center gap-1.5">
                  <span class="truncate text-[13px] font-semibold">{{ item.site }}</span>
                  <KeyRound
                    v-if="item.protected"
                    class="size-3 shrink-0 text-muted-foreground"
                    aria-label="Password protected"
                  />
                </span>
                <span class="mt-1 flex items-center gap-1.5 text-xs">
                  <span :class="cn('size-1.5 shrink-0 rounded-full', stateDotClass(item.state))" aria-hidden="true" />
                  <span :class="cn('truncate', item.host ? 'font-mono text-muted-foreground' : 'text-faint')">
                    {{ hostText(item) }}
                  </span>
                </span>
              </span>
            </button>
          </div>
        </template>

        <template v-else>
          <p class="flex h-[30px] items-end px-2.5 pb-1.5 text-[11px] font-medium uppercase tracking-[0.6px] text-muted-foreground">
            {{ sec.label }}
          </p>
          <button
            v-for="item in sec.items"
            :key="item.key"
            type="button"
            :data-key="item.key"
            :class="
              cn(
                TABLE_COLS,
                'h-[50px] w-full rounded-lg px-2.5 text-left transition-colors hover:bg-accent/60 focus-visible:bg-brand/25 focus-visible:outline-none',
              )
            "
            @click="emit('select', item.key)"
          >
            <span class="flex min-w-0 items-center gap-1.5">
              <span class="truncate text-[13px] font-semibold">{{ item.site }}</span>
              <KeyRound
                v-if="item.protected"
                class="size-3 shrink-0 text-muted-foreground"
                aria-label="Password protected"
              />
            </span>
            <span class="truncate text-xs text-muted-foreground">{{ typeFacts(item.kind).title }}</span>
            <span :class="cn('truncate text-xs', item.host ? 'font-mono text-muted-foreground' : 'text-faint')">
              {{ hostText(item) }}
            </span>
            <span class="flex items-center gap-1.5 text-xs">
              <span :class="cn('size-1.5 shrink-0 rounded-full', stateDotClass(item.state))" aria-hidden="true" />
              <span :class="item.state === 'failed' ? 'text-destructive' : 'text-foreground'">{{ stateLabel(item.state) }}</span>
            </span>
            <ChevronRight class="size-4 text-faint" aria-hidden="true" />
          </button>
        </template>
      </section>
    </div>
  </div>
</template>
