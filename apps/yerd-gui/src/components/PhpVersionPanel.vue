<script setup lang="ts">
import { computed, nextTick, ref, useTemplateRef, watch } from "vue";
import {
  Check,
  FileText,
  MoreHorizontal,
  Pencil,
  Plus,
  Puzzle,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-vue-next";

import PhpSettingsGrid, { type SettingsGridRow } from "@/components/php/PhpSettingsGrid.vue";
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
import { SECTION_LABEL } from "@/components/ui/styles";
import { useResource } from "@/composables/useResource";
import { useStagedField } from "@/composables/useStagedField";
import { useToast } from "@/composables/useToast";
import {
  IpcError,
  listPhp,
  listPhpExtensions,
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
  changedCount,
  DISPLAY_ERRORS_HINT,
  directiveNameProblem,
  directiveValueProblem,
  displayErrorsOptions,
  displayErrorsPick,
  effectiveValue,
  FFI_HINT,
  FFI_KEY,
  FFI_SHORT_LABEL,
  GRID_ORDER,
  LIBVIPS_HINT,
  POOL_DEFAULT,
  POOL_HINT,
  POOL_MAX_CHILDREN_KEY,
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
import { cn } from "@/lib/utils";

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

/** True while any extension row's menu is open, so the drawer leaves Escape to it. */
const layerOpen = defineModel<boolean>("layerOpen", { default: false });

const phpCache = useResource("php", listPhp, { immediate: false });
const extensionsCache = useResource("php-extensions", listPhpExtensions, { immediate: false });

/**
 * Write a reply into the shared cache, then hand it to the view. The cache
 * outlives this panel, so a reply that lands after the drawer closed still
 * reaches a reopened panel.
 */
function publish(r: PhpVersionsResponse): void {
  phpCache.mutate(() => r);
  emit("updated", r);
}

// Names the single in-flight mutation, so the control that started it can show
// its own spinner. Every other control keys off `isBusy` instead: this panel's
// requests all rewrite the same version's config, and each success reseeds the
// form from the daemon's reply, so a second mutation landing mid-flight could
// clobber the first's result or discard edits made while it was away.
const busy = ref<string | null>(null);
const isBusy = computed(() => busy.value !== null);

/** Counts the panel's mutations, so a late recovery read can tell it was overtaken. */
let mutationSeq = 0;
watch(
  busy,
  (b) => {
    if (b !== null) mutationSeq += 1;
  },
  { flush: "sync" },
);

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

const inheritedDisplayErrors = computed(() =>
  effectiveValue(props.globalSettings, {}, "display_errors"),
);

const ffiOptions = computed(() => {
  const inherited = effectiveValue(props.globalSettings, {}, FFI_KEY) ?? "Off";
  return [
    { value: "", label: `inherit (${inherited})` },
    { value: "On", label: "On" },
    { value: "Off", label: "Off" },
  ];
});

const showLibvipsHint = computed(
  () =>
    props.libvipsFound === false &&
    effectiveValue(props.globalSettings, form.value, FFI_KEY) === "On",
);

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

/** Whether Save changes has anything to send. */
const canSave = computed(
  () => (settingsDirty.value || poolDirty.value) && !poolProblem.value && !isBusy.value,
);

/**
 * Persist the settings grid: the allowlisted overrides first (every field, so a
 * blank one removes its override), then the worker ceiling. Each reseeds from
 * its reply. A failure stops there and leaves the rest staged.
 */
async function save(): Promise<void> {
  if (!canSave.value) return;
  busy.value = "save";
  try {
    let settingsSaved = false;
    if (settingsDirty.value) {
      try {
        const payload: Record<string, string> = { ...form.value };
        if (!props.ffiAvailable) delete payload[FFI_KEY];
        const r = await setPhpVersionSettings(props.version, payload);
        seed(r.version_settings?.[props.version] ?? {});
        publish(r);
        settingsSaved = true;
      } catch (e) {
        toast.error(`Couldn't update PHP ${props.version} settings`, (e as IpcError).message);
        return;
      }
    }
    if (poolDirty.value) {
      try {
        const r = await setPhpPoolSettings(props.version, {
          max_children: poolMaxChildren.value.trim(),
        });
        seedPool(r.pool?.[props.version] ?? {});
        publish(r);
      } catch (e) {
        const message = (e as IpcError).message;
        toast.error(
          `Couldn't update the PHP ${props.version} pool size`,
          settingsSaved ? `The other settings were saved. ${message}` : message,
        );
        return;
      }
    }
    toast.success(`PHP ${props.version} settings saved`, "The pool restarts to apply the changes.");
  } finally {
    busy.value = null;
  }
}

/** Stage a blank for every grid row, so Save makes the version inherit everything again. */
function resetToShared(): void {
  for (const k of SETTING_KEYS) form.value[k] = "";
  poolMaxChildren.value = "";
}

const changed = computed(() => changedCount(form.value, poolMaxChildren.value, props.ffiAvailable));

const gridRows = computed<SettingsGridRow[]>(() =>
  GRID_ORDER.filter((k) => k !== FFI_KEY || props.ffiAvailable).map((key): SettingsGridRow => {
    if (key === POOL_MAX_CHILDREN_KEY) {
      return {
        key,
        label: "FPM workers",
        hint: POOL_HINT,
        kind: "text",
        value: poolMaxChildren.value,
        placeholder: POOL_DEFAULT,
        changed: poolMaxChildren.value.trim() !== "",
        problem: poolProblem.value,
      };
    }
    const value = form.value[key] ?? "";
    if (key === "display_errors") {
      const inherited = inheritedDisplayErrors.value;
      return {
        key,
        label: "Display errors",
        hint: DISPLAY_ERRORS_HINT,
        kind: "toggle",
        value: value || inherited || "",
        options: displayErrorsOptions(inherited),
        changed: value !== "",
      };
    }
    if (key === FFI_KEY) {
      return { key, label: FFI_SHORT_LABEL, hint: FFI_HINT, kind: "ffi", value, changed: value !== "" };
    }
    const s = TEXT_SETTINGS.find((t) => t.key === key);
    return {
      key,
      label: s?.label ?? key,
      hint: s?.hint ?? "",
      kind: "text",
      value,
      placeholder: effectiveValue(props.globalSettings, {}, key) ?? "PHP default",
      unit: s?.unit,
      changed: value.trim() !== "",
    };
  }),
);

function onGridUpdate(key: string, value: string): void {
  if (key === POOL_MAX_CHILDREN_KEY) poolMaxChildren.value = value;
  else if (key === "display_errors") {
    form.value.display_errors = displayErrorsPick(value, inheritedDisplayErrors.value);
  } else form.value[key] = value;
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

function modeOn(m: XdebugMode): boolean {
  return xdebugModes.value.includes(m);
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

/** How long the post-failure state read may take before the controls unlock anyway. */
const RECOVERY_READ_MS = 10_000;

/**
 * After a failed save the outcome is unknown when the bridge timed out or lost
 * the daemon, so fetch the daemon's current state before the controls unlock.
 * The read is bounded: a daemon that accepts connections but never answers
 * would otherwise keep every PHP control locked. A read that lands after the
 * timeout is still applied, unless another save started in the meantime, since
 * its snapshot may predate that save. A read that times out or fails leaves
 * the state unconfirmed, and the user is told so.
 */
async function reloadAfterFailure(): Promise<void> {
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timedOut = new Promise<"timeout">((resolve) => {
    timer = setTimeout(() => resolve("timeout"), RECOVERY_READ_MS);
  });
  const read = listPhp().catch(() => null);
  const r = await Promise.race([read, timedOut]);
  clearTimeout(timer);
  if (r !== "timeout" && r !== null) {
    publish(r);
    return;
  }
  if (r === "timeout") {
    const seq = mutationSeq;
    void read.then((late) => {
      if (late && seq === mutationSeq) publish(late);
    });
  }
  toast.info(
    `Couldn't confirm PHP ${props.version}'s saved Xdebug state`,
    r === "timeout"
      ? "Yerd didn't answer in time. Refresh to see the current settings."
      : "Yerd couldn't read the current settings. Refresh to try again.",
  );
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
    publish(r);
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
    publish(r);
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
/** Whether the add-directive row was opened with `+ Add directive` (or an extension's menu). */
const addOpen = ref(false);
const showAddRow = computed(() => addOpen.value || dirName.value !== "" || dirValue.value !== "");
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
    addOpen.value = false;
    toast.success(`Set ${name} for PHP ${props.version}`);
    publish(r);
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
    publish(r);
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
    publish(r);
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
  addOpen.value = true;
  dirName.value = `${ext.name}.`;
  void nextTick(() => dirNameInput.value?.focus());
}

function openAddRow(): void {
  addOpen.value = true;
  void nextTick(() => dirNameInput.value?.focus());
}

function closeAddRow(): void {
  addOpen.value = false;
  dirName.value = "";
  dirValue.value = "";
}

const extMenuOpen = ref<Record<string, boolean>>({});

watch(
  extMenuOpen,
  (menus) => {
    layerOpen.value = Object.values(menus).some(Boolean);
  },
  { deep: true },
);

async function removeExtension(name: string): Promise<void> {
  if (isBusy.value) return;
  busy.value = `ext-remove:${name}`;
  try {
    const map = await removePhpExtension(props.version, name);
    extensionsCache.mutate(() => map);
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

const CARD = "min-w-0 rounded-[10px] border bg-card p-4";
const LINK =
  "mt-3 inline-flex items-center gap-1.5 text-xs font-medium text-brand hover:underline disabled:pointer-events-none disabled:opacity-50";
const FIELD =
  "h-7 rounded-md border bg-background px-2 font-mono text-xs focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:opacity-50";

function discard(): void {
  seed(props.overrides);
  seedPool(props.pool);
  seedXdebug(props.xdebug);
  closeAddRow();
  cancelEdit();
}
</script>

<template>
  <div class="flex flex-1 flex-col">
    <template v-if="installedVersion">
      <div class="flex h-6 items-center justify-between gap-3 px-1">
        <h3 :class="SECTION_LABEL">Settings</h3>
        <p class="flex items-center gap-1.5 text-xs text-faint" data-testid="changed-count">
          <template v-if="changed > 0">
            <span class="size-1.5 rounded-full bg-brand" aria-hidden="true" />
            {{ changed }} changed for {{ version }}
            <span aria-hidden="true">·</span>
            <button
              type="button"
              class="font-medium text-brand hover:underline disabled:opacity-50"
              :disabled="isBusy"
              data-testid="reset-to-shared"
              @click="resetToShared"
            >
              Reset to shared
            </button>
          </template>
          <template v-else>Same as shared</template>
        </p>
      </div>

      <PhpSettingsGrid
        class="mt-2"
        :rows="gridRows"
        markers
        :disabled="isBusy"
        :id-prefix="`set-${version}`"
        :aria-suffix="` for PHP ${version}`"
        @update="onGridUpdate"
      >
        <template #ffi>
          <Select
            class="w-[120px]"
            :model-value="form[FFI_KEY] ?? ''"
            :options="ffiOptions"
            :disabled="isBusy"
            :aria-label="`ffi.enable for PHP ${version}`"
            @update:model-value="(v: string) => (form[FFI_KEY] = v)"
          />
        </template>
      </PhpSettingsGrid>
      <p
        v-if="showLibvipsHint"
        class="mt-2 flex items-center gap-1.5 px-1 text-xs text-muted-foreground"
        data-testid="libvips-hint"
      >
        <TriangleAlert class="size-3.5 shrink-0 text-warning" />
        {{ LIBVIPS_HINT }}
      </p>
    </template>

    <p v-else class="text-[13px] text-muted-foreground">
      PHP {{ version }} is no longer installed. Remove these registrations, or reinstall the
      version to configure it again.
    </p>

    <div :class="cn('mt-5 grid gap-3', installedVersion ? 'grid-cols-2' : 'grid-cols-1')">
      <section :class="CARD" data-testid="extensions-card">
        <p class="flex items-center gap-2 text-[13px] font-semibold">
          <Puzzle class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          Extra extensions
        </p>
        <p class="mt-1 text-xs text-faint">Load a .so into web and CLI for {{ version }} only.</p>

        <div v-if="extensionsLoading && !sortedExtensions.length" class="mt-3 flex">
          <Spinner class="size-4" />
        </div>
        <ul v-else-if="sortedExtensions.length" class="mt-3 flex flex-col gap-1.5">
          <li
            v-for="ext in sortedExtensions"
            :key="ext.name"
            class="flex items-center gap-2 rounded-md border bg-background px-2.5 py-1.5"
          >
            <div class="min-w-0 flex-1">
              <p class="flex items-center gap-1.5">
                <span class="truncate font-mono text-xs font-semibold">{{ ext.name }}</span>
                <Badge v-if="ext.zend" variant="secondary">zend</Badge>
                <Badge v-if="!ext.present" variant="destructive">
                  <TriangleAlert class="size-3" /> missing
                </Badge>
              </p>
              <p class="mt-0.5 truncate font-mono text-[11px] text-faint" :title="ext.path">
                {{ ext.path }}
              </p>
              <p v-if="!ext.present" class="mt-0.5 text-[11px] text-muted-foreground">
                The file has moved or been deleted - remove this registration and add it again
                with the new path.
              </p>
            </div>
            <DropdownMenu
              :open="extMenuOpen[ext.name] ?? false"
              @update:open="(o: boolean) => (extMenuOpen[ext.name] = o)"
            >
              <DropdownMenuTrigger as-child>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-7 shrink-0"
                  :disabled="isBusy"
                  :aria-label="`Actions for ${ext.name}`"
                >
                  <Spinner v-if="busy === `ext-remove:${ext.name}`" class="size-4" />
                  <MoreHorizontal v-else />
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
          </li>
        </ul>

        <button
          v-if="installedVersion"
          type="button"
          :class="LINK"
          :disabled="isBusy"
          @click="emit('requestAddExtension')"
        >
          <Plus class="size-3.5" /> Add extension
        </button>
      </section>

      <section v-if="installedVersion" :class="CARD" data-testid="directives-card">
        <p class="flex items-center gap-2 text-[13px] font-semibold">
          <FileText class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
          Other ini directives
        </p>
        <p class="mt-1 text-xs text-faint">Anything not listed above, e.g. opcache.enable.</p>
        <p
          v-if="xdebugEnabled"
          class="mt-1.5 text-[11px] text-muted-foreground"
          data-testid="xdebug-directives-hint"
        >
          {{ XDEBUG_DIRECTIVES_HINT }}
        </p>

        <ul v-if="directiveEntries.length" class="mt-3 flex flex-col gap-1.5">
          <li v-for="[name, value] in directiveEntries" :key="name">
            <div class="flex items-center gap-1 rounded-md border bg-background px-2.5 py-1">
              <template v-if="editName === name">
                <code class="shrink-0 text-xs">{{ name }} =</code>
                <input
                  v-model="editValue"
                  :class="FIELD"
                  class="min-w-0 flex-1"
                  :disabled="isBusy"
                  :aria-label="`New value for ${name}`"
                  @keydown.enter="saveEdit"
                  @keydown.esc.stop="cancelEdit"
                />
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-7 shrink-0"
                  :disabled="isBusy || !!editProblem || !editValue"
                  :aria-label="`Save ${name}`"
                  @click="saveEdit"
                >
                  <Spinner v-if="busy === `dir-edit:${name}`" class="size-4" />
                  <Check v-else />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-7 shrink-0"
                  :disabled="isBusy"
                  :aria-label="`Cancel editing ${name}`"
                  @click="cancelEdit"
                >
                  <X />
                </Button>
              </template>
              <template v-else>
                <code class="min-w-0 flex-1 truncate text-xs" :title="`${name} = ${value}`">
                  {{ name }} = {{ value }}
                </code>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-7 shrink-0"
                  :disabled="isBusy"
                  :aria-label="`Edit ${name}`"
                  @click="startEdit(name, value)"
                >
                  <Pencil />
                </Button>
                <Button
                  variant="ghost"
                  size="icon"
                  class="size-7 shrink-0"
                  :disabled="isBusy"
                  :aria-label="`Remove ${name}`"
                  @click="removeDirective(name)"
                >
                  <Spinner v-if="busy === `dir-remove:${name}`" class="size-4" />
                  <Trash2 v-else />
                </Button>
              </template>
            </div>
            <p v-if="editName === name && editProblem" class="mt-1 text-[11px] text-destructive">
              {{ editProblem }}
            </p>
          </li>
        </ul>

        <div v-if="showAddRow" class="mt-3" data-testid="directive-add-row">
          <div class="grid grid-cols-2 gap-1.5">
            <Input
              ref="dirNameInput"
              v-model="dirName"
              placeholder="Directive name"
              :disabled="isBusy"
              :aria-label="`Directive name for PHP ${version}`"
            />
            <Input
              v-model="dirValue"
              placeholder="Value"
              :disabled="isBusy"
              :aria-label="`Directive value for PHP ${version}`"
            />
          </div>
          <p v-if="dirProblem" class="mt-1 text-[11px] text-destructive">{{ dirProblem }}</p>
          <div class="mt-2 flex justify-end gap-1.5">
            <Button variant="ghost" size="sm" :disabled="isBusy" @click="closeAddRow">Cancel</Button>
            <Button
              size="sm"
              :disabled="isBusy || !!dirProblem || !dirName || !dirValue"
              data-testid="directive-add"
              @click="addDirective"
            >
              <Spinner v-if="busy === 'dir-add'" class="size-4" />
              Add
            </Button>
          </div>
        </div>
        <button v-else type="button" :class="LINK" :disabled="isBusy" @click="openAddRow">
          <Plus class="size-3.5" /> Add directive
        </button>
      </section>
    </div>

    <template v-if="installedVersion">
      <section class="mt-7" data-testid="xdebug-section">
        <div class="flex h-6 items-center justify-between gap-3 px-1">
          <h3 :class="SECTION_LABEL">Xdebug</h3>
          <p
            v-if="xdebugAvailable"
            class="truncate text-xs text-faint"
            :title="XDEBUG_IDE_HINT"
            data-testid="xdebug-ide-hint"
          >
            IDE listens on port 9003 ·
            <button
              type="button"
              class="font-medium text-brand hover:underline"
              @click="openInBrowser(XDEBUG_DOCS_URL)"
            >
              Debugging with Xdebug
            </button>
          </p>
        </div>
        <div class="mt-2 divide-y rounded-[10px] border bg-card">
          <div class="flex items-center gap-4 px-4 py-3">
            <div class="min-w-0 flex-1">
              <p class="text-[13px] font-semibold">Load Xdebug into PHP {{ version }}'s web pool</p>
              <p class="mt-0.5 text-xs text-faint">
                Restarts the pool when toggled. CLI php never loads it — use phpdebug for one
                command.
              </p>
              <p
                v-if="!xdebugAvailable"
                class="mt-1 text-xs text-muted-foreground"
                data-testid="xdebug-unavailable"
              >
                {{ XDEBUG_UNAVAILABLE_HINT }}
              </p>
            </div>
            <Spinner v-if="busy === 'xdebug-toggle'" class="size-4 shrink-0" />
            <Switch
              :model-value="xdebugEnabled"
              :disabled="isBusy || (!xdebugAvailable && !xdebugEnabled)"
              :aria-label="`Xdebug for PHP ${version}`"
              @update:model-value="toggleXdebug"
            />
          </div>
          <div class="flex items-start gap-4 px-4 py-3">
            <span class="w-12 shrink-0 pt-1 text-xs text-muted-foreground">Modes</span>
            <div class="flex flex-wrap gap-1.5" role="group" :aria-label="`Xdebug modes for PHP ${version}`">
              <button
                v-for="m in XDEBUG_MODES"
                :key="m.value"
                type="button"
                :aria-pressed="modeOn(m.value)"
                :aria-label="`Xdebug ${m.value} mode for PHP ${version}`"
                :title="m.note ? `${m.label} (${m.value}), ${m.note}` : `${m.label} (${m.value})`"
                :disabled="isBusy || !xdebugAvailable || modeLocked(m.value)"
                :data-mode="m.value"
                :class="
                  cn(
                    'inline-flex h-[26px] items-center gap-1.5 rounded-md border px-2.5 text-xs transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed',
                    modeOn(m.value)
                      ? 'border-brand/60 bg-brand/15 font-medium text-foreground'
                      : 'bg-background text-muted-foreground hover:text-foreground',
                    (!xdebugAvailable || (isBusy && !modeLocked(m.value))) && 'opacity-50',
                  )
                "
                @click="setMode(m.value, !modeOn(m.value))"
              >
                <Check v-if="modeOn(m.value)" class="size-3 text-brand" aria-hidden="true" />
                {{ m.short ?? m.label }}
                <span v-if="m.note" class="text-[11px] font-medium text-warning">slow</span>
              </button>
            </div>
          </div>
          <div class="flex items-center gap-4 px-4 py-3">
            <span class="w-12 shrink-0 text-xs text-muted-foreground">Start</span>
            <Select
              class="min-w-0 max-w-[360px] flex-1"
              :model-value="xdebugStart"
              :options="XDEBUG_START_OPTIONS"
              :disabled="isBusy || !xdebugAvailable"
              :aria-label="`Xdebug start for PHP ${version}`"
              @update:model-value="(v: XdebugStart) => startField.edit(v)"
            />
            <Button
              variant="outline"
              size="sm"
              data-testid="xdebug-apply"
              :disabled="!xdebugDirty || isBusy || !xdebugAvailable"
              @click="saveXdebug"
            >
              <Spinner v-if="busy === 'xdebug'" class="size-4" />
              {{ busy === "xdebug" ? "Applying…" : "Apply" }}
            </Button>
          </div>
        </div>
      </section>

      <div class="mt-auto pt-8">
        <div class="flex items-center gap-2 border-t pt-4">
          <span class="mr-auto text-xs text-faint">Saving restarts PHP {{ version }}'s pool only</span>
          <Button v-if="panelDirty" variant="ghost" :disabled="isBusy" @click="discard">Discard</Button>
          <Button
            class="h-[34px] px-4 text-[13px] font-semibold"
            :disabled="!canSave"
            data-testid="version-save"
            @click="save"
          >
            <Spinner v-if="busy === 'save'" class="size-4" />
            {{ busy === "save" ? "Applying…" : "Save changes" }}
          </Button>
        </div>
      </div>
    </template>
  </div>
</template>
