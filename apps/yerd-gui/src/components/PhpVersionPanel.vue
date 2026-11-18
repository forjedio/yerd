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
import Switch from "@/components/ui/Switch.vue";
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip";
import { useStagedField } from "@/composables/useStagedField";
import { useToast } from "@/composables/useToast";
import {
  IpcError,
  listPhp,
  openInBrowser,
  type PhpExtensionsMap,
  removePhpExtension,
  setPhpDirectives,
  setPhpPoolSettings,
  setPhpVersionSettings,
  setXdebug,
} from "@/ipc/client";
import type {
  PhpExtInfo,
  PhpVersion,
  PhpVersionsResponse,
  XdebugMode,
  XdebugSettings,
  XdebugStart,
} from "@/ipc/types";
import {
  DISPLAY_ERRORS_HINT,
  directiveNameProblem,
  directiveValueProblem,
  effectiveValue,
  FFI_HINT,
  FFI_KEY,
  FFI_LABEL,
  LIBVIPS_HINT,
  overrideCount,
  poolMaxChildrenProblem,
  SETTING_KEYS,
  TEXT_SETTINGS,
  XDEBUG_DEFAULT,
  XDEBUG_DIRECTIVES_HINT,
  XDEBUG_DOCS_URL,
  XDEBUG_IDE_HINT,
  XDEBUG_MODES,
  XDEBUG_START_OPTIONS,
  XDEBUG_UNAVAILABLE_HINT,
} from "@/lib/phpSettings";

const props = defineProps<{
  version: PhpVersion;
  /** The global `[php.settings]` defaults this version inherits from. */
  globalSettings: Record<string, string>;
  /** This version's sparse setting overrides (may be empty). */
  overrides: Record<string, string>;
  /** This version's free-form ini directives (may be empty). */
  directives: Record<string, string>;
  /** This version's FPM pool overrides (may be empty; empty means default). */
  pool: Record<string, string>;
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
  /** Whether this version's PHP build includes the FFI extension. */
  ffiAvailable: boolean;
  /** Whether libvips was found on the host; `null` when not checked. */
  libvipsFound: boolean | null;
  /** This version's Xdebug settings; `null` means the default (off). */
  xdebug: XdebugSettings | null;
  /** Whether a bundled Xdebug is published for this version on this host. */
  xdebugAvailable: boolean;
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

// Names the single in-flight mutation, so the control that started it can show
// its own spinner. Every other control keys off `isBusy` instead: this panel's
// requests all rewrite the same version's config, and each success reseeds the
// form from the daemon's reply, so a second mutation landing mid-flight could
// clobber the first's result or discard edits made while it was away.
const busy = ref<string | null>(null);
const isBusy = computed(() => busy.value !== null);

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

const ffiOptions = computed(() => {
  const inherited = effectiveValue(props.globalSettings, {}, FFI_KEY) ?? "Off";
  return [
    { value: "", label: `- inherit (${inherited}) -` },
    { value: "On", label: "On" },
    { value: "Off", label: "Off" },
  ];
});

const showLibvipsHint = computed(
  () =>
    props.libvipsFound === false &&
    effectiveValue(props.globalSettings, form.value, FFI_KEY) === "On",
);

const savedOverrides = computed(() => overrideCount(props.overrides));

/**
 * Persist this version's overrides. Every field is sent, including the blank
 * ones: a blank value removes the override so the version inherits again.
 */
async function saveSettings(): Promise<void> {
  if (isBusy.value) return;
  busy.value = "settings";
  try {
    const payload: Record<string, string> = { ...form.value };
    if (!props.ffiAvailable) delete payload[FFI_KEY];
    const r = await setPhpVersionSettings(props.version, payload);
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

// ── FPM pool sizing ──
// Same pristine/seed discipline as the settings grid: an empty field means the
// built-in default, and server refreshes only reseed while the field is clean.
const poolMaxChildren = ref("");
const poolSeeded = ref("");

function seedPool(pool: Record<string, string>): void {
  poolMaxChildren.value = pool["max_children"] ?? "";
  poolSeeded.value = poolMaxChildren.value;
}

const poolDirty = computed(
  () => poolMaxChildren.value.trim() !== poolSeeded.value.trim(),
);

const poolProblem = computed(() => poolMaxChildrenProblem(poolMaxChildren.value));

watch(
  () => props.pool,
  (p) => {
    if (!poolDirty.value) seedPool(p);
  },
  { immediate: true },
);

async function savePool(): Promise<void> {
  if (isBusy.value || poolProblem.value) return;
  busy.value = "pool";
  try {
    const r = await setPhpPoolSettings(props.version, {
      max_children: poolMaxChildren.value.trim(),
    });
    seedPool(r.pool?.[props.version] ?? {});
    toast.success(
      `PHP ${props.version} pool size updated`,
      "The pool restarts to apply the change.",
    );
    emit("updated", r);
  } catch (e) {
    toast.error(`Couldn't update the PHP ${props.version} pool size`, (e as IpcError).message);
  } finally {
    busy.value = null;
  }
}

// ── Xdebug ──
// The switch applies at once. Modes and the start trigger are staged behind
// Apply; each field reconciles with daemon refreshes on its own, and a save
// sends only the fields the user edited so the daemon keeps the rest.
const xdebugEnabled = computed(() => props.xdebug?.enabled ?? false);
const modesField = useStagedField<XdebugMode[]>([...XDEBUG_DEFAULT.modes], (v) =>
  v.join(","),
);
const startField = useStagedField<XdebugStart>(XDEBUG_DEFAULT.start);
const { draft: xdebugModes } = modesField;
const { draft: xdebugStart } = startField;

const xdebugDirty = computed(() => modesField.dirty.value || startField.dirty.value);

function syncXdebug(x: XdebugSettings | null): void {
  const s = x ?? XDEBUG_DEFAULT;
  modesField.sync([...s.modes]);
  startField.sync(s.start);
}

function seedXdebug(x: XdebugSettings | null): void {
  const s = x ?? XDEBUG_DEFAULT;
  modesField.reset([...s.modes]);
  startField.reset(s.start);
}

watch(() => props.xdebug, syncXdebug, { immediate: true });

/** Whether the mode's checkbox is locked: the last checked mode can't be cleared. */
function modeLocked(m: XdebugMode): boolean {
  return xdebugModes.value.length === 1 && xdebugModes.value[0] === m;
}

function setMode(m: XdebugMode, checked: boolean): void {
  if (!checked && modeLocked(m)) return;
  const next = new Set(xdebugModes.value);
  if (checked) next.add(m);
  else next.delete(m);
  modesField.edit(XDEBUG_MODES.map((o) => o.value).filter((v) => next.has(v)));
}

/** The staged modes and start, each only when the user edited it. */
function editedXdebug(): [XdebugMode[] | undefined, XdebugStart | undefined] {
  return [
    modesField.dirty.value ? [...xdebugModes.value] : undefined,
    startField.dirty.value ? xdebugStart.value : undefined,
  ];
}

/**
 * After a failed save the outcome is unknown when the bridge timed out or lost
 * the daemon, so fetch the daemon's current state before the controls unlock.
 */
async function reloadAfterFailure(): Promise<void> {
  const r = await listPhp().catch(() => null);
  if (r) emit("updated", r);
}

/**
 * Flip the switch. Turning it on while the mode/start form has unapplied edits
 * sends the edited fields with the enable, so the pool starts with what the
 * user sees; turning it off sends only the switch and keeps the staged edits.
 */
async function toggleXdebug(on: boolean): Promise<void> {
  if (isBusy.value) return;
  busy.value = "xdebug-toggle";
  const withStaged = on && xdebugDirty.value;
  try {
    const r = withStaged
      ? await setXdebug(props.version, true, ...editedXdebug())
      : await setXdebug(props.version, on);
    if (withStaged) seedXdebug(r.xdebug?.[props.version] ?? null);
    toast.success(`Xdebug ${on ? "on" : "off"} for PHP ${props.version}`, "Settings saved.");
    emit("updated", r);
  } catch (e) {
    toast.error(
      `Couldn't turn Xdebug ${on ? "on" : "off"} for PHP ${props.version}`,
      (e as IpcError).message,
    );
    await reloadAfterFailure();
  } finally {
    busy.value = null;
  }
}

async function saveXdebug(): Promise<void> {
  if (isBusy.value) return;
  busy.value = "xdebug";
  try {
    const r = await setXdebug(props.version, null, ...editedXdebug());
    const saved = r.xdebug?.[props.version] ?? null;
    seedXdebug(saved);
    toast.success(
      `PHP ${props.version} Xdebug settings saved`,
      saved?.enabled ? undefined : "They apply the next time Xdebug is turned on.",
    );
    emit("updated", r);
  } catch (e) {
    toast.error(`Couldn't update PHP ${props.version} Xdebug settings`, (e as IpcError).message);
    await reloadAfterFailure();
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
  if (isBusy.value) return;
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
  if (isBusy.value) return;
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
  if (isBusy.value) return;
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
  if (isBusy.value) return;
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
    poolDirty.value ||
    xdebugDirty.value ||
    dirName.value !== "" ||
    dirValue.value !== "" ||
    editName.value !== null
  );
});

watch(panelDirty, (d) => emit("dirty", d), { immediate: true });

function discard(): void {
  seed(props.overrides);
  seedPool(props.pool);
  seedXdebug(props.xdebug);
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
              :disabled="isBusy"
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
                :disabled="isBusy"
                :aria-label="`display_errors for PHP ${version}`"
                @update:model-value="(v: string) => (form.display_errors = v)"
              />
            </div>
          </div>
          <div v-if="ffiAvailable" data-testid="ffi-setting">
            <div class="flex items-center gap-1">
              <span class="text-xs font-medium">{{ FFI_LABEL }}</span>
              <Tooltip>
                <TooltipTrigger as-child>
                  <span class="inline-flex cursor-help text-muted-foreground">
                    <Info class="size-3.5" />
                  </span>
                </TooltipTrigger>
                <TooltipContent side="top">{{ FFI_HINT }}</TooltipContent>
              </Tooltip>
            </div>
            <div class="mt-1">
              <Select
                class="w-full"
                :model-value="form[FFI_KEY] ?? ''"
                :options="ffiOptions"
                :disabled="isBusy"
                :aria-label="`ffi.enable for PHP ${version}`"
                @update:model-value="(v: string) => (form[FFI_KEY] = v)"
              />
            </div>
            <p
              v-if="showLibvipsHint"
              class="mt-1 flex items-center gap-1.5 text-xs text-muted-foreground"
              data-testid="libvips-hint"
            >
              <TriangleAlert class="size-3.5 shrink-0 text-warning" />
              {{ LIBVIPS_HINT }}
            </p>
          </div>
        </div>
      </TooltipProvider>

      <!-- Xdebug, loaded into this version's web (FPM) pool only. -->
      <div class="mt-5 border-t border-border pt-4" data-testid="xdebug-section">
        <div class="flex items-center justify-between gap-2">
          <TooltipProvider :delay-duration="0">
            <div class="flex items-center gap-1">
              <span class="text-xs font-medium">Xdebug</span>
              <Tooltip>
                <TooltipTrigger as-child>
                  <span class="inline-flex cursor-help text-muted-foreground">
                    <Info class="size-3.5" />
                  </span>
                </TooltipTrigger>
                <TooltipContent>
                  Loads the bundled Xdebug into this version's web pool. Turning
                  it on or off restarts the pool. Plain php on the command line
                  never loads it; use phpdebug for a single command.
                </TooltipContent>
              </Tooltip>
            </div>
          </TooltipProvider>
          <span class="flex items-center gap-2">
            <Spinner v-if="busy === 'xdebug-toggle'" class="size-4" />
            <Switch
              :model-value="xdebugEnabled"
              :disabled="isBusy || (!xdebugAvailable && !xdebugEnabled)"
              :aria-label="`Xdebug for PHP ${version}`"
              @update:model-value="toggleXdebug"
            />
          </span>
        </div>
        <p
          v-if="!xdebugAvailable"
          class="mt-1 text-xs text-muted-foreground"
          data-testid="xdebug-unavailable"
        >
          {{ XDEBUG_UNAVAILABLE_HINT }}
        </p>
        <fieldset class="mt-3" :disabled="!xdebugAvailable">
          <legend class="text-xs text-muted-foreground">Modes</legend>
          <div class="mt-1 grid grid-cols-1 gap-x-4 gap-y-1.5 sm:grid-cols-2">
            <label
              v-for="m in XDEBUG_MODES"
              :key="m.value"
              class="flex items-center gap-2 text-sm"
            >
              <input
                type="checkbox"
                class="size-4"
                :checked="xdebugModes.includes(m.value)"
                :disabled="isBusy || !xdebugAvailable || modeLocked(m.value)"
                :aria-label="`Xdebug ${m.value} mode for PHP ${version}`"
                @change="setMode(m.value, ($event.target as HTMLInputElement).checked)"
              />
              {{ m.label }}
              <span class="text-xs text-muted-foreground">
                {{ m.note ? `${m.value}, ${m.note}` : m.value }}
              </span>
            </label>
          </div>
        </fieldset>
        <div class="mt-3 flex items-start gap-2">
          <Select
            class="w-72"
            :model-value="xdebugStart"
            :options="XDEBUG_START_OPTIONS"
            :disabled="isBusy || !xdebugAvailable"
            :aria-label="`Xdebug start for PHP ${version}`"
            @update:model-value="(v: XdebugStart) => startField.edit(v)"
          />
          <Button
            size="sm"
            data-testid="xdebug-apply"
            :disabled="!xdebugDirty || isBusy || !xdebugAvailable"
            @click="saveXdebug"
          >
            <Spinner v-if="busy === 'xdebug'" class="size-4" />
            {{ busy === "xdebug" ? "Applying…" : "Apply" }}
          </Button>
        </div>
        <p
          v-if="xdebugAvailable"
          class="mt-1 text-xs text-muted-foreground"
          data-testid="xdebug-ide-hint"
        >
          {{ XDEBUG_IDE_HINT }} See
          <button
            type="button"
            class="text-brand hover:underline"
            @click="openInBrowser(XDEBUG_DOCS_URL)"
          >
            Debugging with Xdebug</button>.
        </p>
      </div>

      <!-- FPM pool sizing. Web (FPM) only: these are pool-block settings, so
           they never reach this version's CLI ini. -->
      <div class="mt-5 border-t border-border pt-4" data-testid="pool-section">
        <TooltipProvider :delay-duration="0">
          <div class="flex items-center gap-1">
            <label class="text-xs font-medium" :for="`pool-${version}-max-children`">
              FPM pool size
            </label>
            <Tooltip>
              <TooltipTrigger as-child>
                <span class="inline-flex cursor-help text-muted-foreground">
                  <Info class="size-3.5" />
                </span>
              </TooltipTrigger>
              <TooltipContent>
                Most PHP workers this version may run at once (pm.max_children,
                1-1024). The pool is on-demand, so a higher ceiling costs
                nothing while idle. Raise it if requests queue behind queue
                workers, parallel tests, or many open tabs.
              </TooltipContent>
            </Tooltip>
          </div>
        </TooltipProvider>
        <div class="mt-1 flex items-start gap-2">
          <Input
            :id="`pool-${version}-max-children`"
            v-model="poolMaxChildren"
            type="number"
            min="1"
            max="1024"
            placeholder="16 (default)"
            class="max-w-40"
            :disabled="isBusy"
          />
          <Button
            size="sm"
            :disabled="!poolDirty || isBusy || poolProblem !== null"
            @click="savePool"
          >
            <Spinner v-if="busy === 'pool'" class="size-4" />
            {{ busy === "pool" ? "Applying…" : "Apply" }}
          </Button>
        </div>
        <p v-if="poolProblem" class="mt-1 text-xs text-destructive">
          {{ poolProblem }}
        </p>
        <p v-else class="mt-1 text-xs text-muted-foreground">
          Empty means the default of 16. Changing this restarts the pool.
        </p>
      </div>

      <div class="mt-4 flex items-center justify-between gap-2">
        <span class="text-xs text-muted-foreground">
          {{ savedOverrides }} saved override{{ savedOverrides === 1 ? "" : "s" }} ·
          empty fields inherit
        </span>
        <span class="flex items-center gap-2">
          <Button
            variant="ghost"
            size="sm"
            :disabled="!panelDirty || isBusy"
            @click="discard"
          >
            Discard
          </Button>
          <Button size="sm" :disabled="!settingsDirty || isBusy" @click="saveSettings">
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
        <Button
          v-if="installedVersion"
          :disabled="isBusy"
          @click="emit('requestAddExtension')"
        >
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
                :disabled="isBusy"
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
              Free-form directives for this version, e.g. opcache.validate_timestamps = 0.
              Names and values are checked for safety; whether a directive means
              anything is up to PHP and its extensions.
            </TooltipContent>
          </Tooltip>
        </TooltipProvider>
      </div>

      <p
        v-if="xdebugEnabled"
        class="mt-1 text-xs text-muted-foreground"
        data-testid="xdebug-directives-hint"
      >
        {{ XDEBUG_DIRECTIVES_HINT }}
      </p>

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
                :disabled="isBusy"
                :aria-label="`New value for ${name}`"
                @keydown.enter="saveEdit"
                @keydown.esc="cancelEdit"
              />
              <Button
                variant="ghost"
                size="sm"
                :disabled="isBusy || !!editProblem || !editValue"
                :aria-label="`Save ${name}`"
                @click="saveEdit"
              >
                <Spinner v-if="busy === `dir-edit:${name}`" class="size-4" />
                <Check v-else class="size-4" />
              </Button>
              <Button
                variant="ghost"
                size="sm"
                :disabled="isBusy"
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
                  :disabled="isBusy"
                  :aria-label="`Edit ${name}`"
                  @click="startEdit(name, value)"
                >
                  <Pencil class="size-4" />
                </Button>
                <Button
                  variant="ghost"
                  size="sm"
                  :disabled="isBusy"
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
          :disabled="isBusy"
          :aria-label="`Directive name for PHP ${version}`"
        />
        <Input
          v-model="dirValue"
          placeholder="Value"
          class="flex-1"
          :disabled="isBusy"
          :aria-label="`Directive value for PHP ${version}`"
        />
        <Button
          :disabled="isBusy || !!dirProblem || !dirName || !dirValue"
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
