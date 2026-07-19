<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Check, ChevronDown, Info, Pencil, Plus, Trash2, X } from "lucide-vue-next";

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
import { IpcError, setPhpDirectives, setPhpVersionSettings } from "@/ipc/client";
import type { PhpVersion, PhpVersionsResponse } from "@/ipc/types";
import {
  DISPLAY_ERRORS_HINT,
  directiveNameProblem,
  directiveValueProblem,
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
  /** This version's free-form ini directives (may be empty). */
  directives: Record<string, string>;
}>();

const emit = defineEmits<{
  /** Fired with the daemon's refreshed version list after any successful save. */
  (e: "updated", r: PhpVersionsResponse): void;
}>();

const toast = useToast();
const open = ref(false);
const busy = ref<string | null>(null);

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
const directiveEntries = computed(() =>
  Object.entries(props.directives).sort(([a], [b]) => a.localeCompare(b)),
);

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
  busy.value = "settings";
  try {
    // Send every field; blank values remove the override (inherit again).
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
    busy.value = null;
  }
}

// ── custom ini directives ──
const dirName = ref("");
const dirValue = ref("");

// Inline hint while typing; the daemon remains the authority on save.
const dirProblem = computed(() => {
  if (!dirName.value && !dirValue.value) return null;
  return directiveNameProblem(dirName.value) ?? directiveValueProblem(dirValue.value);
});

async function addDirective(): Promise<void> {
  const name = dirName.value.trim();
  const value = dirValue.value.trim();
  if (directiveNameProblem(name) || directiveValueProblem(value)) {
    toast.error("Invalid directive", dirProblem.value ?? "check the name and value");
    return;
  }
  busy.value = "dir-add";
  try {
    const r = await setPhpDirectives(props.version, { [name]: value });
    dirName.value = "";
    dirValue.value = "";
    toast.success(`Set ${name} for PHP ${props.version}`);
    emit("updated", r);
  } catch (e) {
    toast.error("Couldn't set the directive", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// Inline editing of one directive's value. The name stays fixed; renaming is
// remove + add. Saving reuses the set path, which upserts on the daemon.
const editName = ref<string | null>(null);
const editValue = ref("");

const editProblem = computed(() =>
  editName.value === null || editValue.value === ""
    ? null
    : directiveValueProblem(editValue.value),
);

function startEdit(name: string, value: string): void {
  editName.value = name;
  editValue.value = value;
}

function cancelEdit(): void {
  editName.value = null;
  editValue.value = "";
}

async function saveEdit(): Promise<void> {
  const name = editName.value;
  if (name === null) return;
  const value = editValue.value.trim();
  if (value === "" || directiveValueProblem(value)) return;
  busy.value = `dir-edit:${name}`;
  try {
    const r = await setPhpDirectives(props.version, { [name]: value });
    cancelEdit();
    toast.success(`Updated ${name} for PHP ${props.version}`);
    emit("updated", r);
  } catch (e) {
    toast.error("Couldn't update the directive", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

async function removeDirective(name: string): Promise<void> {
  busy.value = `dir-remove:${name}`;
  try {
    const r = await setPhpDirectives(props.version, { [name]: "" });
    toast.success(`Removed ${name} for PHP ${props.version}`);
    emit("updated", r);
  } catch (e) {
    toast.error("Couldn't remove the directive", (e as IpcError).message);
  } finally {
    busy.value = null;
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
        <Badge v-if="directiveEntries.length" variant="secondary">
          {{ directiveEntries.length }} directive{{ directiveEntries.length === 1 ? "" : "s" }}
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
        <Button size="sm" :disabled="busy === 'settings'" @click="saveSettings">
          <Spinner v-if="busy === 'settings'" class="size-4" />
          {{ busy === "settings" ? "Applying…" : "Save" }}
        </Button>
      </div>

      <div class="mt-5 border-t border-border pt-4">
        <div class="flex items-center gap-1">
          <span class="text-xs font-medium">Custom ini directives</span>
          <TooltipProvider :delay-duration="0">
            <Tooltip>
              <TooltipTrigger as-child>
                <span class="inline-flex cursor-help text-muted-foreground">
                  <Info class="size-3.5" />
                </span>
              </TooltipTrigger>
              <TooltipContent side="top">
                Free-form directives for this version, e.g. xdebug.mode = debug.
                Names and values are checked for safety; whether a directive means
                anything is up to PHP and its extensions.
              </TooltipContent>
            </Tooltip>
          </TooltipProvider>
        </div>

        <div v-if="directiveEntries.length" class="mt-2 flex flex-col gap-2">
          <template v-for="[name, value] in directiveEntries" :key="name">
            <div
              class="flex items-center justify-between gap-2 rounded-md border border-border px-3 py-1.5"
            >
              <template v-if="editName === name">
                <code class="shrink-0 text-xs">{{ name }} =</code>
                <Input
                  v-model="editValue"
                  class="flex-1"
                  :aria-label="`New value for ${name}`"
                  @keydown.enter="saveEdit"
                  @keydown.esc="cancelEdit"
                />
                <Button
                  variant="ghost"
                  size="sm"
                  :disabled="busy === `dir-edit:${name}` || !!editProblem || !editValue"
                  :aria-label="`Save ${name}`"
                  @click="saveEdit"
                >
                  <Spinner v-if="busy === `dir-edit:${name}`" class="size-4" />
                  <Check v-else class="size-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  :aria-label="`Cancel editing ${name}`"
                  @click="cancelEdit"
                >
                  <X class="size-4" />
                </Button>
              </template>
              <template v-else>
                <code class="truncate text-xs">{{ name }} = {{ value }}</code>
                <span class="flex items-center">
                  <Button
                    variant="ghost"
                    size="sm"
                    :aria-label="`Edit ${name}`"
                    @click="startEdit(name, value)"
                  >
                    <Pencil class="size-4" />
                  </Button>
                  <Button
                    variant="ghost"
                    size="sm"
                    :disabled="busy === `dir-remove:${name}`"
                    :aria-label="`Remove ${name}`"
                    @click="removeDirective(name)"
                  >
                    <Spinner v-if="busy === `dir-remove:${name}`" class="size-4" />
                    <Trash2 v-else class="size-4" />
                  </Button>
                </span>
              </template>
            </div>
            <p v-if="editName === name && editProblem" class="text-xs text-destructive">
              {{ editProblem }}
            </p>
          </template>
        </div>
        <p v-else class="mt-2 text-xs text-muted-foreground">
          No custom directives for this version yet.
        </p>

        <div class="mt-3 flex items-start gap-2">
          <Input
            v-model="dirName"
            placeholder="Directive name"
            class="flex-1"
            :aria-label="`Directive name for PHP ${version}`"
          />
          <Input
            v-model="dirValue"
            placeholder="Value"
            class="flex-1"
            :aria-label="`Directive value for PHP ${version}`"
          />
          <Button
            size="sm"
            :disabled="busy === 'dir-add' || !!dirProblem || !dirName || !dirValue"
            @click="addDirective"
          >
            <Spinner v-if="busy === 'dir-add'" class="size-4" />
            <Plus v-else class="size-4" />
            Add
          </Button>
        </div>
        <p v-if="dirProblem" class="mt-1 text-xs text-destructive">{{ dirProblem }}</p>
      </div>
    </div>
  </div>
</template>
