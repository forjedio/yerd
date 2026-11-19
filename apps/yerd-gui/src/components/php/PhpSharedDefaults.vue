<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { RotateCw, TriangleAlert } from "lucide-vue-next";

import PhpSettingsGrid, { type SettingsGridRow } from "@/components/php/PhpSettingsGrid.vue";
import Button from "@/components/ui/Button.vue";
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
const props = defineProps<{
  data: PhpVersionsResponse | null;
  /** Whether Restart all pools is in flight. */
  restarting: boolean;
  /** Whether any pool is running (or failed), so there is something to restart. */
  anyRunning: boolean;
}>();

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

function applySettings(settings: Record<string, string> | undefined): void {
  const next: Record<string, string> = {};
  for (const s of TEXT_SETTINGS) next[s.key] = settings?.[s.key] ?? "";
  next.display_errors = settings?.display_errors ?? "";
  next[FFI_KEY] = settings?.[FFI_KEY] ?? "";
  settingsForm.value = next;
  lastSeeded.value = { ...next };
}

/** True when the form still matches what was last seeded (no unsaved edits). */
function settingsPristine(): boolean {
  const form = settingsForm.value;
  const seeded = lastSeeded.value;
  const keys = new Set([...Object.keys(form), ...Object.keys(seeded)]);
  for (const k of keys) {
    if ((form[k] ?? "") !== (seeded[k] ?? "")) return false;
  }
  return true;
}

/** Seeds on first load and on later revalidations only while pristine, so an
 *  out-of-band change self-corrects without clobbering unsaved edits. */
watch(
  () => props.data,
  (d) => {
    if (d && settingsPristine()) applySettings(d.settings);
  },
  { immediate: true },
);

const dirty = computed(() => !settingsPristine());

watch(dirty, (d) => emit("dirty", d), { immediate: true });

/** The FFI switch maps On to "On" and off to "" (PHP's own default). It is
 *  disabled when no installed version has FFI, unless the saved value is on, so
 *  a CLI-set value can still be cleared. "No version has FFI" is only claimed
 *  once the list has loaded. */
const ffiVersions = computed<PhpVersion[]>(() => props.data?.ffi_versions ?? []);
const noFfiVersions = computed(() => props.data != null && ffiVersions.value.length === 0);
const ffiOn = computed(() => settingsForm.value[FFI_KEY] === "On");
const ffiSavedOn = computed(() => props.data?.settings?.[FFI_KEY] === "On");
const ffiSwitchDisabled = computed(() => noFfiVersions.value && !ffiSavedOn.value);

function setFfi(on: boolean): void {
  settingsForm.value[FFI_KEY] = on ? "On" : "";
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

/** Send every field; a blank value removes that setting. */
async function saveSettings(): Promise<void> {
  if (saving.value) return;
  saving.value = true;
  try {
    const r = await setPhpSettings({ ...settingsForm.value });
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

    <div class="mt-auto pt-8">
      <div class="flex items-center gap-3 border-t pt-4">
        <span class="mr-auto text-xs text-faint">Saving restarts every running pool</span>
        <Button
          class="h-[34px] px-4 text-[13px] font-semibold"
          :disabled="saving"
          data-testid="shared-save"
          @click="saveSettings"
        >
          <Spinner v-if="saving" class="size-4" />
          {{ saving ? "Applying…" : "Save changes" }}
        </Button>
      </div>
    </div>
  </div>
</template>
