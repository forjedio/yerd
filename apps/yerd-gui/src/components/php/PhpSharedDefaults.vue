<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { RotateCw, TriangleAlert } from "lucide-vue-next";

import PhpSettingsGrid, { type SettingsGridRow } from "@/components/php/PhpSettingsGrid.vue";
import Button from "@/components/ui/Button.vue";
import InspectorFooter from "@/components/ui/InspectorFooter.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { useResource } from "@/composables/useResource";
import { useToast } from "@/composables/useToast";
import { IpcError, listPhp, setPhpSettings } from "@/ipc/client";
import type { PhpVersion, PhpVersionsResponse } from "@/ipc/types";
import {
  DISPLAY_ERRORS_HINT,
  DISPLAY_ERRORS_OPTIONS,
  FFI_HINT,
  FFI_KEY,
  FFI_LABEL,
  FFI_SHORT_LABEL,
  GRID_ORDER,
  LIBVIPS_HINT,
  NO_FFI_VERSIONS_HINT,
  POOL_MAX_CHILDREN_KEY,
  TEXT_SETTINGS,
} from "@/lib/phpSettings";

/**
 * The Shared defaults pane: the global `[php.settings]` every installed version
 * starts from, in the settings grid without change markers, plus the FFI switch
 * and Restart all pools. A blank field means PHP's own default (the daemon
 * removes the key). Saving restarts every running pool.
 */
const props = withDefaults(defineProps<{
  data: PhpVersionsResponse | null;
  /** Whether Restart all pools is in flight. */
  restarting: boolean;
  /** Whether any pool is running (or failed), so there is something to restart. */
  anyRunning: boolean;
  /** Whether this panel is the one on show, so its footer bar is too. */
  active?: boolean;
}>(), { active: true });

const emit = defineEmits<{
  /** Fired with the daemon's refreshed state after a successful save. */
  updated: [r: PhpVersionsResponse];
  restartAll: [];
  /** Whether the form holds unsaved edits. */
  dirty: [isDirty: boolean];
}>();

const toast = useToast();
const { mutate } = useResource("php", listPhp, { immediate: false });

const settingsForm = ref<Record<string, string>>({});
const lastSeeded = ref<Record<string, string>>({});
const saving = ref(false);

function seededFrom(settings: Record<string, string> | undefined): Record<string, string> {
  const next: Record<string, string> = {};
  for (const s of TEXT_SETTINGS) next[s.key] = settings?.[s.key] ?? "";
  next.display_errors = settings?.display_errors ?? "";
  next[FFI_KEY] = settings?.[FFI_KEY] ?? "";
  return next;
}

/** Reset the whole form to `settings`. */
function applySettings(settings: Record<string, string> | undefined): void {
  const next = seededFrom(settings);
  settingsForm.value = next;
  lastSeeded.value = { ...next };
}

/** Move the baseline to `settings`, carrying the user's edits over it: an
 *  untouched field takes the fresh value, an edited one keeps the edit. So an
 *  out-of-band change (CLI, MCP) to another key self-corrects even while the
 *  form is dirty, and a later Save can't revert it. */
function rebase(settings: Record<string, string> | undefined): void {
  const next = seededFrom(settings);
  const form: Record<string, string> = {};
  for (const [k, v] of Object.entries(next)) {
    const current = settingsForm.value[k] ?? "";
    form[k] = current === (lastSeeded.value[k] ?? "") ? v : current;
  }
  settingsForm.value = form;
  lastSeeded.value = next;
}

/** The fields whose value differs from the baseline, as a save payload. */
function changedSettings(): Record<string, string> {
  const changed: Record<string, string> = {};
  for (const [k, v] of Object.entries(settingsForm.value)) {
    if (v !== (lastSeeded.value[k] ?? "")) changed[k] = v;
  }
  return changed;
}

/** True when the form still matches what was last seeded (no unsaved edits). */
function settingsPristine(): boolean {
  return Object.keys(changedSettings()).length === 0;
}

watch(
  () => props.data,
  (d) => {
    if (d) rebase(d.settings);
  },
  { immediate: true },
);

const dirty = computed(() => !settingsPristine());

watch(dirty, (d) => emit("dirty", d), { immediate: true });

/** The FFI switch maps On to "On" and off to the saved off value ("" for PHP's
 *  own default, or an explicit "Off"), so toggling back leaves the form clean. It is
 *  disabled when no installed version has FFI, unless the saved value is on, so
 *  a CLI-set value can still be cleared. "No version has FFI" is only claimed
 *  once the list has loaded. */
const ffiVersions = computed<PhpVersion[]>(() => props.data?.ffi_versions ?? []);
const noFfiVersions = computed(() => props.data != null && ffiVersions.value.length === 0);
const ffiOn = computed(() => settingsForm.value[FFI_KEY] === "On");
const ffiSavedOn = computed(() => props.data?.settings?.[FFI_KEY] === "On");
const ffiSwitchDisabled = computed(() => noFfiVersions.value && !ffiSavedOn.value);

function setFfi(on: boolean): void {
  const saved = lastSeeded.value[FFI_KEY] ?? "";
  settingsForm.value[FFI_KEY] = on ? "On" : saved === "On" ? "" : saved;
}

const rows = computed<SettingsGridRow[]>(() =>
  GRID_ORDER.filter((k) => k !== POOL_MAX_CHILDREN_KEY).map((key): SettingsGridRow => {
    const value = settingsForm.value[key] ?? "";
    if (key === "display_errors") {
      return {
        key,
        label: "Display errors",
        hint: DISPLAY_ERRORS_HINT,
        kind: "toggle",
        value,
        options: DISPLAY_ERRORS_OPTIONS,
      };
    }
    if (key === FFI_KEY) {
      return { key, label: FFI_SHORT_LABEL, hint: FFI_HINT, kind: "ffi", value };
    }
    const s = TEXT_SETTINGS.find((t) => t.key === key);
    return {
      key,
      label: s?.label ?? key,
      hint: s?.hint ?? "",
      kind: "text",
      value,
      placeholder: s?.placeholder,
      unit: s?.unit,
    };
  }),
);

function onUpdate(key: string, value: string): void {
  settingsForm.value[key] = value;
}

/** Send only the edited fields (the daemon merges them); a blank value removes
 *  that setting. Untouched keys are left to whatever the daemon holds now. */
async function saveSettings(): Promise<void> {
  if (saving.value) return;
  const payload = changedSettings();
  if (Object.keys(payload).length === 0) return;
  saving.value = true;
  try {
    const r = await setPhpSettings(payload);
    applySettings(r.settings);
    mutate(() => r);
    toast.success("PHP settings updated", "Pools restart to apply the changes.");
    emit("updated", r);
  } catch (e) {
    toast.error("Couldn't update PHP settings", (e as IpcError).message);
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <div class="flex flex-1 flex-col" data-testid="shared-defaults">
    <div class="flex items-start gap-3">
      <div class="min-w-0 flex-1">
        <h2 class="text-xl font-semibold tracking-[-0.2px]">Shared defaults</h2>
        <p class="mt-1 text-xs text-faint">
          Every installed version starts from these. Leave blank for PHP's own default.
        </p>
      </div>
      <Button
        variant="outline"
        class="h-[34px] shrink-0 gap-1.5 px-3 text-[13px] font-semibold"
        :disabled="!anyRunning || restarting"
        :title="anyRunning ? 'Restart every running PHP pool' : 'No pools are running'"
        @click="emit('restartAll')"
      >
        <Spinner v-if="restarting" class="size-4" />
        <RotateCw v-else />
        Restart all pools
      </Button>
      <slot name="header-extra" />
    </div>

    <h3 class="mt-6 px-1 text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">Settings</h3>

    <PhpSettingsGrid
      class="mt-2"
      :rows="rows"
      :markers="false"
      :disabled="saving"
      id-prefix="set"
      @update="onUpdate"
    >
      <template #ffi>
        <Switch
          :model-value="ffiOn"
          :disabled="ffiSwitchDisabled || saving"
          :aria-label="FFI_LABEL"
          data-testid="toggle-ffi"
          @update:model-value="setFfi"
        />
      </template>
    </PhpSettingsGrid>

    <div class="mt-2 space-y-1 px-1" data-testid="ffi-global">
      <p v-if="noFfiVersions" class="text-xs text-faint" data-testid="ffi-unavailable">
        {{ NO_FFI_VERSIONS_HINT }}
      </p>
      <p
        v-if="ffiOn && data?.libvips_found === false"
        class="flex items-center gap-1.5 text-xs text-muted-foreground"
        data-testid="libvips-hint"
      >
        <TriangleAlert class="size-3.5 shrink-0 text-warning" />
        {{ LIBVIPS_HINT }}
      </p>
    </div>

    <InspectorFooter :active="active">
      <template #start>Saving restarts every running pool</template>
      <Button
        class="h-[34px] px-4 text-[13px] font-semibold"
        :disabled="saving"
        data-testid="shared-save"
        @click="saveSettings"
      >
        <Spinner v-if="saving" class="size-4" />
        {{ saving ? "Applying…" : "Save changes" }}
      </Button>
    </InspectorFooter>
  </div>
</template>
