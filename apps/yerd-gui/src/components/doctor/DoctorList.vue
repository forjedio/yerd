<script setup lang="ts">
import { CircleCheck, CircleHelp, CircleX, Info, TriangleAlert, Wrench } from "lucide-vue-next";
import type { Component } from "vue";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { Severity } from "@/ipc/types";
import { type FindingGroup, permissionCopy, permissionKey } from "@/lib/doctor";
import type { PrivilegeItem } from "@/lib/elevation";
import { cn } from "@/lib/utils";

/**
 * The Doctor list: findings grouped Problems / Warnings / Info, then the
 * SYSTEM PERMISSIONS rows. Each row is a tinted icon tile, a title and a
 * one-line subtitle; the selected row drives the inspector beside it.
 */
defineProps<{
  groups: FindingGroup[];
  /** True until the first diagnose result arrives. */
  loading: boolean;
  items: PrivilegeItem[];
  selected: string | null;
  /** Show Fix all on the SYSTEM PERMISSIONS label. */
  canFixAll: boolean;
  /** The in-flight elevation key, e.g. `elevate:all`. */
  busy: string | null;
}>();

const emit = defineEmits<{ select: [key: string]; fixAll: [] }>();

const SEVERITY_TILE: Record<Severity, { icon: Component; class: string }> = {
  fail: { icon: CircleX, class: "border-destructive/25 bg-destructive/10 text-destructive" },
  warn: { icon: TriangleAlert, class: "border-warning/25 bg-warning/10 text-warning" },
  ok: { icon: Info, class: "border-brand/25 bg-brand/10 text-brand" },
};

function permissionTile(value: boolean | null): { icon: Component; class: string } {
  if (value === true) return { icon: CircleCheck, class: "border-success/25 bg-success/10 text-success" };
  if (value === false) return SEVERITY_TILE.warn;
  return { icon: CircleHelp, class: "bg-muted text-faint" };
}

const LABEL = "px-2 pb-1.5 pt-4 text-[11px] font-semibold uppercase tracking-[0.6px] text-faint";
const ROW =
  "flex h-[58px] w-full items-center gap-3 rounded-lg border px-2.5 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring";
const TILE = "flex size-8 shrink-0 items-center justify-center rounded-md border";

function rowClass(selected: boolean): string {
  return cn(ROW, selected ? "border-brand/45 bg-brand/[0.14]" : "border-transparent hover:bg-accent/60");
}
</script>

<template>
  <div class="min-h-0 flex-1 overflow-y-auto px-3.5 pb-3.5" data-testid="doctor-list">
    <div v-if="loading" class="flex justify-center py-8"><Spinner class="size-5" /></div>
    <template v-else>
      <section
        v-for="group in groups"
        :key="group.severity"
        role="group"
        :aria-label="group.label"
        :data-group="group.severity"
      >
        <p :class="LABEL">{{ group.label }} · {{ group.items.length }}</p>
        <div class="flex flex-col gap-1">
          <button
            v-for="{ key, finding } in group.items"
            :key="key"
            type="button"
            :data-key="key"
            :aria-current="selected === key ? 'true' : undefined"
            :class="rowClass(selected === key)"
            @click="emit('select', key)"
          >
            <span :class="cn(TILE, SEVERITY_TILE[finding.severity].class)">
              <component :is="SEVERITY_TILE[finding.severity].icon" class="size-4" aria-hidden="true" />
            </span>
            <span class="min-w-0 flex-1">
              <span class="block truncate text-[13px] font-semibold">{{ finding.title }}</span>
              <span class="mt-0.5 block truncate text-xs text-muted-foreground">{{ finding.detail }}</span>
            </span>
          </button>
        </div>
      </section>
    </template>

    <section v-if="items.length" role="group" aria-label="System permissions" data-group="permissions">
      <div class="flex items-end justify-between">
        <p :class="LABEL">System permissions</p>
        <Button
          v-if="canFixAll"
          variant="ghost"
          size="sm"
          class="mb-0.5 h-6 gap-1 px-2 text-xs"
          :disabled="busy !== null"
          data-testid="doctor-fix-all"
          @click="emit('fixAll')"
        >
          <Spinner v-if="busy === 'elevate:all'" class="size-3.5" />
          <Wrench v-else class="size-3.5" />
          Fix all
        </Button>
      </div>
      <div class="flex flex-col gap-1">
        <button
          v-for="item in items"
          :key="item.key"
          type="button"
          :data-key="permissionKey(item.target)"
          :aria-current="selected === permissionKey(item.target) ? 'true' : undefined"
          :class="rowClass(selected === permissionKey(item.target))"
          @click="emit('select', permissionKey(item.target))"
        >
          <span :class="cn(TILE, permissionTile(item.value).class)">
            <component :is="permissionTile(item.value).icon" class="size-4" aria-hidden="true" />
          </span>
          <span class="min-w-0 flex-1">
            <span class="block truncate text-[13px] font-semibold">{{ item.label }}</span>
            <span class="mt-0.5 block truncate text-xs text-muted-foreground">{{ permissionCopy(item).summary }}</span>
          </span>
          <Spinner v-if="busy?.endsWith(`:${item.target}`)" class="size-3.5 shrink-0" />
        </button>
      </div>
    </section>
  </div>
</template>
