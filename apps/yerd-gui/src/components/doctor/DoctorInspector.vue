<script setup lang="ts">
import {
  CircleCheck,
  CircleHelp,
  Copy,
  KeyRound,
  RefreshCw,
  ShieldCheck,
  Undo2,
  Unplug,
  Wrench,
} from "lucide-vue-next";
import { computed } from "vue";

import ComingSoon from "@/components/ComingSoon.vue";
import { CARD } from "@/components/ui/styles";
import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { Diagnosis, ElevateTarget, Severity } from "@/ipc/types";
import { permissionCopy, SEVERITY_CHIP } from "@/lib/doctor";
import type { PrivilegeItem } from "@/lib/elevation";
import { CHIP_TONE, cn } from "@/lib/utils";

/** What the Doctor inspector shows. */
export type DoctorInspectorState = "loading" | "finding" | "permission" | "clear" | "error";

/**
 * The Doctor inspector beside the list: a finding (severity chip, detail and a
 * copyable remedy), a system permission (status chip, description and the
 * Fix / Revert card), or the loading, all-clear and unavailable states.
 */
const props = defineProps<{
  state: DoctorInspectorState;
  finding: Diagnosis | null;
  item: PrivilegeItem | null;
  canElevate: boolean;
  /** The in-flight elevation key, e.g. `elevate:trust`. */
  busy: string | null;
  /** A diagnose run is in flight. */
  checking: boolean;
}>();

const emit = defineEmits<{
  copy: [text: string];
  fix: [target: ElevateTarget];
  revert: [target: ElevateTarget];
  recheck: [];
}>();

const SEVERITY_CHIP_CLASS: Record<Severity, string> = {
  fail: CHIP_TONE.danger,
  warn: CHIP_TONE.warning,
  ok: CHIP_TONE.brand,
};

const copy = computed(() => (props.item ? permissionCopy(props.item) : null));

const permissionTone = computed(() => {
  const v = props.item?.value;
  if (v === true) return { chip: CHIP_TONE.success, tile: "bg-success/10 text-success", icon: ShieldCheck };
  if (v === false) return { chip: CHIP_TONE.warning, tile: "bg-warning/10 text-warning", icon: KeyRound };
  return { chip: CHIP_TONE.neutral, tile: "bg-muted text-faint", icon: CircleHelp };
});

const CHIP = "shrink-0 rounded px-1.5 py-0.5 text-[11px] font-semibold uppercase tracking-[0.4px]";
const TITLE = "min-w-0 truncate text-xl font-semibold leading-tight tracking-[-0.2px]";
const CARD_LABEL = "text-[11px] font-medium uppercase tracking-[0.6px] text-faint";
</script>

<template>
  <div class="flex h-full min-h-0 flex-col overflow-y-auto" data-testid="doctor-inspector" :data-state="state">
    <div v-if="state === 'loading'" class="flex flex-1 items-center justify-center"><Spinner class="size-5" /></div>

    <div v-else-if="state === 'finding' && finding" class="p-7">
      <h2 class="flex items-center gap-2.5">
        <span :class="TITLE">{{ finding.title }}</span>
        <span :class="cn(CHIP, SEVERITY_CHIP_CLASS[finding.severity])" data-testid="doctor-chip">
          {{ SEVERITY_CHIP[finding.severity] }}
        </span>
      </h2>
      <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{{ finding.detail }}</p>

      <div v-if="finding.remedy" :class="cn(CARD, 'mt-6 p-[18px]')" data-testid="doctor-remedy">
        <p :class="CARD_LABEL">Fix it yourself</p>
        <div class="mt-2.5 flex min-h-[50px] items-center gap-3 rounded-md border bg-background py-2 pl-3.5 pr-2">
          <code class="min-w-0 flex-1 break-all font-mono text-[13px]">{{ finding.remedy }}</code>
          <Button
            variant="outline"
            size="icon"
            class="size-[30px] shrink-0"
            aria-label="Copy command"
            @click="emit('copy', finding.remedy)"
          >
            <Copy class="size-3.5" />
          </Button>
        </div>
      </div>
    </div>

    <div v-else-if="state === 'permission' && item && copy" class="p-7">
      <h2 class="flex items-center gap-2.5">
        <span :class="TITLE">{{ item.label }}</span>
        <span :class="cn(CHIP, permissionTone.chip)" data-testid="doctor-chip">{{ copy.chip }}</span>
      </h2>
      <p class="mt-2 text-[13px] leading-relaxed text-muted-foreground">{{ copy.description }}</p>

      <div :class="cn(CARD, 'mt-6 flex items-start gap-3.5 p-[18px]')" data-testid="doctor-permission-card">
        <span :class="cn('flex size-9 shrink-0 items-center justify-center rounded-lg', permissionTone.tile)">
          <component :is="permissionTone.icon" class="size-4" aria-hidden="true" />
        </span>
        <div class="min-w-0 flex-1">
          <p class="text-[14px] font-semibold">{{ copy.cardTitle }}</p>
          <p class="mt-1 text-[13px] leading-relaxed text-muted-foreground">{{ copy.cardBody }}</p>
          <p v-if="item.note" class="mt-2 text-xs text-muted-foreground" data-testid="doctor-note">{{ item.note }}</p>
        </div>
        <template v-if="item.fixable">
          <Button
            v-if="canElevate"
            size="sm"
            class="shrink-0"
            :disabled="busy !== null"
            @click="emit('fix', item.target)"
          >
            <Spinner v-if="busy === `elevate:${item.target}`" class="size-4" />
            <Wrench v-else />
            Fix
          </Button>
          <ComingSoon
            v-else
            reason="In-app elevation isn't available on this platform yet - use `yerd elevate` in a terminal for now."
            pill
          >
            Fix
          </ComingSoon>
        </template>
        <Button
          v-else-if="item.unelevatable && canElevate"
          variant="outline"
          size="sm"
          class="shrink-0"
          :disabled="busy !== null"
          @click="emit('revert', item.target)"
        >
          <Spinner v-if="busy === `unelevate:${item.target}`" class="size-4" />
          <Undo2 v-else />
          Revert
        </Button>
      </div>

      <p class="mt-4 flex items-center gap-2 text-xs text-faint">
        <ShieldCheck class="size-3.5 shrink-0" aria-hidden="true" />
        Fixes run the audited yerd elevate helper under an OS prompt. The app itself never runs as root.
      </p>
    </div>

    <div v-else-if="state === 'error'" class="flex flex-1 flex-col items-center justify-center px-6 text-center">
      <Unplug class="size-6 text-destructive" aria-hidden="true" />
      <p class="mt-3 text-[15px] font-semibold">Health check unavailable</p>
      <p class="mt-1.5 text-[13px] text-muted-foreground">Couldn't fetch diagnostics from the daemon.</p>
      <Button variant="outline" size="sm" class="mt-4" :disabled="checking" @click="emit('recheck')">
        <Spinner v-if="checking" class="size-4" />
        <RefreshCw v-else />
        Re-check
      </Button>
    </div>

    <div v-else class="flex flex-1 flex-col items-center justify-center px-6 text-center">
      <CircleCheck class="size-6 text-success" aria-hidden="true" />
      <p class="mt-3 text-[15px] font-semibold">No problems found</p>
      <p class="mt-1.5 max-w-xs text-[13px] text-muted-foreground">
        Your Yerd environment looks healthy. System permissions are listed alongside.
      </p>
    </div>
  </div>
</template>
