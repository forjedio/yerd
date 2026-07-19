<script setup lang="ts">
import { computed, nextTick, ref, useTemplateRef, watch } from "vue";
import {
  Check,
  Info,
  MoreHorizontal,
  Pencil,
  Plus,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-vue-next";

import Badge from "@/components/ui/Badge.vue";
import Button from "@/components/ui/Button.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
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
import {
  IpcError,
  type PhpExtensionsMap,
  removePhpExtension,
  setPhpDirectives,
  setPhpVersionSettings,
} from "@/ipc/client";
import type { PhpExtInfo, PhpVersion, PhpVersionsResponse } from "@/ipc/types";
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
  /** This version's registered custom extensions (may be empty). */
  extensions: PhpExtInfo[];
  /**
   * False for a version that still has registered extensions but is no longer
   * installed. The daemon refuses every write for such a version except
   * removing an extension, so the panel offers only that.
   */
  installedVersion: boolean;
  /** True while the extensions resource is still loading for the first time. */
  extensionsLoading: boolean;
}>();

const emit = defineEmits<{
  /** Fired with the daemon's refreshed version list after any successful save. */
  (e: "updated", r: PhpVersionsResponse): void;
  /** Fired with the daemon's refreshed extension map after a remove. */
  (e: "extensionsUpdated", map: PhpExtensionsMap): void;
  /** Mirrors `panelDirty` so the tab strip can mark unsaved work. */
  (e: "dirty", isDirty: boolean): void;
  /** Asks the view to open its single add-extension modal for this version. */
  (e: "requestAddExtension"): void;
}>();

const toast = useToast();
const busy = ref<string | null>(null);

// ── per-version settings form ──
// Fields hold only the override value; an empty field means "inherit" (the
// placeholder shows what is inherited). Same pristine/seed discipline as the
// global settings form: server refreshes only reseed while there are no
// unsaved edits.
const form = ref<Record<string, string>>({});
const lastSeeded = ref<Record<string, string>>({});

function seed(overrides: Record<string, string>): void {
  const next: Record<string, string> = {};
  for (const k of SETTING_KEYS) next[k] = overrides[k] ?? "";
  form.value = next;
  lastSeeded.value = { ...next };
}

/** Unsaved edits in the settings grid alone. */
const settingsDirty = computed(() =>
  SETTING_KEYS.some((k) => (form.value[k] ?? "") !== (lastSeeded.value[k] ?? "")),
);

// Guarded on `settingsDirty` rather than the wider `panelDirty`: a half-typed
// directive must not stop an out-of-band ini change (e.g. via the CLI) from
// self-correcting here.
watch(
  () => props.overrides,
  (o) => {
    if (!settingsDirty.value) seed(o);
  },
  { immediate: true },
);

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

const savedOverrides = computed(() => overrideCount(props.overrides));

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
const dirNameInput = useTemplateRef<{ focus: () => void }>("dirNameInput");

// Inline hint while typing; the daemon remains the authority on save. An empty
// value isn't flagged until something is typed into it, so seeding just the
// name (from an extension's menu) doesn't greet the user with a red error.
const dirProblem = computed(() => {
  if (!dirName.value && !dirValue.value) return null;
  const nameProblem = directiveNameProblem(dirName.value);
  if (nameProblem) return nameProblem;
  return dirValue.value === "" ? null : directiveValueProblem(dirValue.value);
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

// ── extensions ──
const sortedExtensions = computed(() =>
  [...props.extensions].sort((a, b) => a.name.localeCompare(b.name)),
);

/**
 * Seed the directive name with the extension's prefix and focus the field. The
 * menu suppresses its own focus restore (`@close-auto-focus.prevent`), which
 * would otherwise pull focus straight back to the trigger.
 */
function directiveFor(ext: PhpExtInfo): void {
  dirName.value = `${ext.name}.`;
  void nextTick(() => dirNameInput.value?.focus());
}

async function removeExtension(name: string): Promise<void> {
  busy.value = `ext-remove:${name}`;
  try {
    const map = await removePhpExtension(props.version, name);
    toast.success("Extension removed");
    emit("extensionsUpdated", map);
  } catch (e) {
    toast.error("Couldn't remove the extension", (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── dirty reporting ──
// Wider than `settingsDirty`: a typed-but-unadded directive is unsaved work
// too, and switching tabs would otherwise hide it without a trace.
const panelDirty = computed(() => {
  if (!props.installedVersion) return false;
  return (
    settingsDirty.value ||
    dirName.value !== "" ||
    dirValue.value !== "" ||
    editName.value !== null
  );
});

watch(panelDirty, (d) => emit("dirty", d), { immediate: true });

function discard(): void {
  seed(props.overrides);
  dirName.value = "";
  dirValue.value = "";
  cancelEdit();
}
</script>

<template>
  <div>
    <template v-if="installedVersion">
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
          {{ savedOverrides }} saved override{{ savedOverrides === 1 ? "" : "s" }} ·
          empty fields inherit
        </span>
        <span class="flex items-center gap-2">
          <Button variant="ghost" size="sm" :disabled="!panelDirty" @click="discard">
            Discard
          </Button>
          <Button
            size="sm"
            :disabled="!settingsDirty || busy === 'settings'"
            @click="saveSettings"
          >
            <Spinner v-if="busy === 'settings'" class="size-4" />
            {{ busy === "settings" ? "Applying…" : "Save changes" }}
          </Button>
        </span>
      </div>
    </template>

    <p v-else class="text-xs text-muted-foreground">
      PHP {{ version }} is no longer installed. Remove these registrations, or
      reinstall the version to configure it again.
    </p>

    <!-- Extensions, loaded into both the web (FPM) and CLI runtimes. -->
    <div :class="installedVersion ? 'mt-5 border-t border-border pt-4' : 'mt-3'">
      <div class="flex items-center justify-between gap-2">
        <span class="text-xs font-medium">Extensions</span>
        <Button v-if="installedVersion" @click="emit('requestAddExtension')">
          <Plus class="size-4" />
          Add…
        </Button>
      </div>

      <div v-if="extensionsLoading && !sortedExtensions.length" class="mt-2 flex py-2">
        <Spinner class="size-4" />
      </div>
      <div v-else-if="sortedExtensions.length" class="mt-2 flex flex-col gap-2">
        <div
          v-for="ext in sortedExtensions"
          :key="ext.name"
          class="flex items-center justify-between gap-2 rounded-md border border-border px-3 py-2"
        >
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <span class="text-sm font-medium">{{ ext.name }}</span>
              <Badge v-if="ext.zend" variant="secondary">zend</Badge>
              <Badge v-if="!ext.present" variant="destructive">
                <TriangleAlert class="size-3" /> missing
              </Badge>
            </div>
            <p class="mt-0.5 truncate text-xs text-muted-foreground">{{ ext.path }}</p>
            <p v-if="!ext.present" class="mt-0.5 text-xs text-muted-foreground">
              The file has moved or been deleted - remove this registration and
              add it again with the new path.
            </p>
          </div>
          <DropdownMenu>
            <DropdownMenuTrigger as-child>
              <Button
                variant="ghost"
                size="sm"
                :disabled="busy === `ext-remove:${ext.name}`"
                :aria-label="`Actions for ${ext.name}`"
              >
                <Spinner v-if="busy === `ext-remove:${ext.name}`" class="size-4" />
                <MoreHorizontal v-else class="size-4" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" @close-auto-focus.prevent>
              <DropdownMenuItem v-if="installedVersion" @select="directiveFor(ext)">
                <Plus class="size-4" /> Add ini directive
              </DropdownMenuItem>
              <DropdownMenuSeparator v-if="installedVersion" />
              <DropdownMenuItem
                class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                @select="removeExtension(ext.name)"
              >
                <Trash2 class="size-4" /> Remove
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
      </div>
      <p v-else class="mt-2 text-xs text-muted-foreground">
        No extensions registered for PHP {{ version }}.
      </p>
    </div>

    <div v-if="installedVersion" class="mt-5 border-t border-border pt-4">
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
          ref="dirNameInput"
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
</template>
