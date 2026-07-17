<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { ChevronDown, Info } from "lucide-vue-next";

import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useToast } from "@/composables/useToast";
import { IpcError, setPhpVersionSettings } from "@/ipc/client";
import type { PhpVersion, PhpVersionsResponse } from "@/ipc/types";
import {
  DISPLAY_ERRORS_HINT,
  effectiveValue,
  overrideCount,
  SETTING_KEYS,
  TEXT_SETTINGS,
} from "@/lib/phpSettings";

const props = defineProps<{
  version: PhpVersion;
  /** The global `[php.settings]` defaults this version inherits from. */
  globalSettings: Record<string, string>;
  /** This version's sparse setting overrides (may be empty). */
  overrides: Record<string, string>;
}>();

const emit = defineEmits<{
  /** Fired with the daemon's refreshed version list after a successful save. */
  (e: "updated", r: PhpVersionsResponse): void;
}>();

const toast = useToast();
const open = ref(false);
const busy = ref(false);

// ── per-version settings form ──
// Fields hold only the override value; an empty field means "inherit" (the
// placeholder shows what is inherited). Same pristine/seed discipline as the
// global settings form: server refreshes only reseed while there are no
// unsaved edits.
const form = ref<Record<string, string>>({});
let lastSeeded: Record<string, string> = {};

function seed(overrides: Record<string, string>): void {
  const next: Record<string, string> = {};
  for (const k of SETTING_KEYS) next[k] = overrides[k] ?? "";
  form.value = next;
  lastSeeded = { ...next };
}

function pristine(): boolean {
  return SETTING_KEYS.every((k) => (form.value[k] ?? "") === (lastSeeded[k] ?? ""));
}

watch(
  () => props.overrides,
  (o) => {
    if (pristine()) seed(o);
  },
  { immediate: true },
);

const badgeCount = computed(() => overrideCount(props.overrides));

function inheritedLabel(key: string, fallback: string): string {
  const inherited = effectiveValue(props.globalSettings, {}, key);
  return `${inherited ?? fallback} (inherited)`;
}

const displayErrorsOptions = computed(() => {
  const inherited = effectiveValue(props.globalSettings, {}, "display_errors");
  return [
    { value: "", label: `- inherit (${inherited ?? "default"}) -` },
    { value: "On", label: "On" },
    { value: "Off", label: "Off" },
  ];
});

async function saveSettings(): Promise<void> {
  busy.value = true;
  try {
    const r = await setPhpVersionSettings(props.version, { ...form.value });
    seed(r.version_settings?.[props.version] ?? {});
    toast.success(
      `PHP ${props.version} settings updated`,
      "The pool restarts to apply the changes.",
    );
    emit("updated", r);
  } catch (e) {
    toast.error(`Couldn't update PHP ${props.version} settings`, (e as IpcError).message);
  } finally {
    busy.value = false;
  }
}
</script>

<template>
  <div class="rounded-lg border border-border">
    <button
      type="button"
      class="flex w-full items-center justify-between px-4 py-3 text-left"
      :aria-expanded="open"
      @click="open = !open"
    >
      <span class="flex items-center gap-2">
        <span class="font-mono text-sm font-medium">PHP {{ version }}</span>
        <Badge v-if="badgeCount" variant="secondary">
          {{ badgeCount }} override{{ badgeCount === 1 ? "" : "s" }}
        </Badge>
      </span>
      <ChevronDown class="size-4 transition-transform" :class="{ 'rotate-180': open }" />
    </button>

    <div v-if="open" class="border-t border-border px-4 py-4">
      <TooltipProvider :delay-duration="0">
        <div class="grid grid-cols-1 gap-4 sm:grid-cols-2">
          <div v-for="s in TEXT_SETTINGS" :key="s.key">
            <div class="flex items-center gap-1">
              <label class="text-xs font-medium" :for="`set-${version}-${s.key}`">
                {{ s.label }}
              </label>
              <Tooltip>
                <TooltipTrigger as-child>
                  <span class="inline-flex cursor-help text-muted-foreground">
                    <Info class="size-3.5" />
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top">{{ s.hint }}</TooltipContent>
              </Tooltip>
            </div>
            <Input
              :id="`set-${version}-${s.key}`"
              v-model="form[s.key]"
              :placeholder="inheritedLabel(s.key, s.placeholder)"
              class="mt-1"
            />
          </div>
          <div>
            <div class="flex items-center gap-1">
              <span class="text-xs font-medium">Display errors</span>
              <Tooltip>
                <TooltipTrigger as-child>
                  <span class="inline-flex cursor-help text-muted-foreground">
                    <Info class="size-3.5" />
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top">{{ DISPLAY_ERRORS_HINT }}</TooltipContent>
              </Tooltip>
            </div>
            <div class="mt-1">
              <Select
                class="w-full"
                :model-value="form.display_errors ?? ''"
                :options="displayErrorsOptions"
                :aria-label="`display_errors for PHP ${version}`"
                @update:model-value="(v: string) => (form.display_errors = v)"
              />
            </div>
          </div>
        </div>
      </TooltipProvider>

      <div class="mt-4 flex items-center justify-between gap-2">
        <span class="text-xs text-muted-foreground">
          Empty fields inherit the default settings above.
        </span>
        <Button size="sm" :disabled="busy" @click="saveSettings">
          <Spinner v-if="busy" class="size-4" />
          {{ busy ? "Applying…" : "Save" }}
        </Button>
      </div>
    </div>
  </div>
</template>
