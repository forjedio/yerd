<script setup lang="ts">
import { Folder, Link2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Input from "@/components/ui/Input.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Select from "@/components/ui/Select.vue";
import SuffixInput from "@/components/ui/SuffixInput.vue";
import Switch from "@/components/ui/Switch.vue";
import { preferredPhp } from "@/composables/useSiteStarters";
import { IpcError, pickDirectory, projectConfigDraft } from "@/ipc/client";
import type { ProjectConfig } from "@/ipc/types";
import { comparePhpVersions } from "@/lib/phpVersion";
import { slugifySiteName } from "@/lib/siteName";
import { tildify, untildify } from "@/lib/sites";

/** What the Link dialog asks the Sites view to do. */
export interface LinkRequest {
  name: string;
  path: string;
  /** PHP version to set after linking; `null` leaves the daemon's default. */
  php: string | null;
  /** HTTPS to set after linking; `null` leaves the daemon's default. */
  secure: boolean | null;
  /** Apply the folder's `.yerd.toml` instead of linking with `php`/`secure`. */
  applyProjectFile: boolean;
}

/**
 * "Link an existing folder": pick or type a project folder, name the site
 * (prefilled from the folder until the user types their own), and choose its
 * PHP version and HTTPS. When the folder has a `.yerd.toml`, the dialog offers
 * to apply it instead: the name comes from the file and PHP/HTTPS are hidden.
 * With the file present but not applied, PHP/HTTPS are hidden too and left at
 * the defaults, because any later change to them is written back into the
 * committed `.yerd.toml`. Emits the request; the Sites view links it and
 * applies the rest.
 */
const props = defineProps<{
  open: boolean;
  tld: string;
  home: string | null;
  phpVersions: string[];
  defaultPhp: string;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  link: [request: LinkRequest];
}>();

/** Above this many versions the segmented control would overflow the row. */
const SEGMENTED_MAX = 6;

/** The folder as shown and typed, possibly `~`-relative. */
const pathText = ref("");
const path = computed(() => untildify(pathText.value.trim(), props.home));
const name = ref("");
/** True once the user types in the name field, so choosing a folder never
 *  clobbers a name they entered themselves. */
const nameDirty = ref(false);
const php = ref("");
const secure = ref(false);

const draft = ref<ProjectConfig | null>(null);
const draftError = ref<string | null>(null);
const applyFile = ref(true);
/** Bumped per lookup so a slow reply for an older folder is dropped. */
let draftSeq = 0;
/** True while a `.yerd.toml` lookup is in flight; Link waits for it. */
const looking = ref(false);
/** The folder the current draft (or its absence) was looked up for. */
const lookedUp = ref<string | null>(null);
/** The lookup in flight, so submit can wait for the one already running. */
let lookup: { dir: string; done: Promise<void> } | null = null;

const hasFile = computed(() => draft.value?.exists === true);
const usingFile = computed(() => hasFile.value && applyFile.value);
/** A `.yerd.toml` is present but the user chose not to apply it. */
const ignoringFile = computed(() => hasFile.value && !applyFile.value);
/** The name the apply job links under: the file's `name`, else the site
 *  already at the folder, else the folder name, as the daemon resolves it. */
const fileName = computed(
  () => draft.value?.name ?? draft.value?.site ?? slugifySiteName(baseName(path.value)),
);

const versions = computed(() => [...props.phpVersions].sort(comparePhpVersions));
const phpOptions = computed(() => versions.value.map((v) => ({ value: v, label: v })));
const phpSelectOptions = computed(() => versions.value.map((v) => ({ value: v, label: `PHP ${v}` })));

/** A folder-derived or typed name is converted to a valid slug on submit, so
 *  validity is "does it slugify to something", not the raw `[a-z0-9-]` shape. */
const valid = computed(() => {
  if (path.value === "") return false;
  return usingFile.value ? fileName.value !== null : slugifySiteName(name.value) !== null;
});

/** Set while a submit waits for its folder's lookup, so a second click
 *  during the wait does not emit twice. */
let submitPending = false;
/** Bumped when the folder changes or the dialog closes, so a submit still
 *  waiting for an older folder's lookup never emits. */
let submitGen = 0;

function abandonSubmit(): void {
  submitGen += 1;
  submitPending = false;
}

watch(
  () => props.open,
  (open) => {
    if (open) {
      php.value = preferredPhp(props.phpVersions, props.defaultPhp, () => true);
      return;
    }
    pathText.value = "";
    name.value = "";
    nameDirty.value = false;
    secure.value = false;
    resetDraft();
    abandonSubmit();
  },
  { immediate: true },
);

function baseName(p: string): string {
  const trimmed = p.replace(/[/\\]+$/, "");
  const parts = trimmed.split(/[/\\]/);
  return parts[parts.length - 1] ?? "";
}

function resetDraft(): void {
  draftSeq += 1;
  draft.value = null;
  draftError.value = null;
  applyFile.value = true;
  looking.value = false;
  lookedUp.value = null;
  lookup = null;
}

/** Look for a `.yerd.toml` in `dir`. Only an invalid file is reported: any
 *  other failure (a folder that doesn't exist yet) is left to the link itself. */
function detectProjectFile(dir: string): Promise<void> {
  resetDraft();
  if (!dir) return Promise.resolve();
  const seq = draftSeq;
  looking.value = true;
  const done = (async () => {
    try {
      const reply = await projectConfigDraft(dir);
      if (seq === draftSeq) draft.value = reply;
    } catch (e) {
      if (seq !== draftSeq) return;
      if (e instanceof IpcError && e.code === "project_file_invalid") draftError.value = e.message;
    } finally {
      if (seq === draftSeq) {
        looking.value = false;
        lookedUp.value = dir;
      }
    }
  })();
  lookup = { dir, done };
  return done;
}

function folderChosen(dir: string): Promise<void> {
  if (!nameDirty.value) name.value = slugifySiteName(baseName(dir)) ?? "";
  return detectProjectFile(dir);
}

/** Resolves once the current folder has been looked up, starting the lookup
 *  when a typed path was never committed (Link clicked before `change`). */
function ensureLookedUp(): Promise<void> {
  const dir = path.value;
  if (lookedUp.value === dir) return Promise.resolve();
  if (lookup?.dir === dir) return lookup.done;
  return folderChosen(dir);
}

async function chooseFolder(): Promise<void> {
  const dir = await pickDirectory(path.value || undefined);
  if (!dir) return;
  abandonSubmit();
  pathText.value = tildify(dir, props.home);
  folderChosen(dir);
}

function typePath(value: string): void {
  abandonSubmit();
  pathText.value = value;
}

function commitPath(): void {
  abandonSubmit();
  folderChosen(path.value);
}

function typeName(value: string): void {
  nameDirty.value = true;
  name.value = value;
}

/** Links the folder. When the click had to wait for the folder's lookup and
 *  that lookup found a `.yerd.toml` (valid or not) the user has not seen yet,
 *  nothing is emitted: the banner shows and a second click is required. */
function submit(): void {
  if (path.value === "" || submitPending) return;
  if (looking.value || lookedUp.value !== path.value) {
    submitPending = true;
    const dir = path.value;
    const gen = submitGen;
    void ensureLookedUp().then(() => {
      if (gen !== submitGen || dir !== path.value) return;
      submitPending = false;
      if (hasFile.value || draftError.value) return;
      finishSubmit();
    });
    return;
  }
  finishSubmit();
}

function finishSubmit(): void {
  if (!valid.value || lookedUp.value !== path.value) return;
  const slug = usingFile.value ? fileName.value : slugifySiteName(name.value);
  if (!slug) return;
  emit("link", {
    name: slug,
    path: path.value,
    php: ignoringFile.value ? null : php.value,
    secure: ignoringFile.value ? null : secure.value,
    applyProjectFile: usingFile.value,
  });
  emit("update:open", false);
}
</script>

<template>
  <DialogFrame
    :open="open"
    title="Link an existing folder"
    @update:open="emit('update:open', $event)"
  >
    <div class="flex items-center gap-3 rounded-[10px] border bg-background px-4 py-3">
      <Folder class="size-5 shrink-0 text-brand" aria-hidden="true" />
      <Input
        class="h-8 min-w-0 flex-1 font-mono text-[13px]"
        :model-value="pathText"
        placeholder="Pick or type the project folder"
        aria-label="Project folder"
        data-testid="link-path"
        @update:model-value="typePath"
        @change="commitPath"
      />
      <Button variant="outline" size="sm" @click="chooseFolder">
        {{ path ? "Change" : "Choose folder…" }}
      </Button>
    </div>

    <InfoBanner v-if="draftError" variant="destructive" class="mt-3" data-testid="project-file-error">
      This folder's <code class="font-mono">.yerd.toml</code> can't be used: {{ draftError }}
    </InfoBanner>
    <InfoBanner v-else-if="hasFile" class="mt-3" data-testid="project-file-banner">
      <div class="flex flex-wrap items-center justify-between gap-x-4 gap-y-1.5">
        <span>This folder has a <code class="font-mono">.yerd.toml</code>.</span>
        <span class="flex items-center gap-2 text-foreground">
          Apply it after linking
          <Switch v-model="applyFile" aria-label="Apply .yerd.toml after linking" />
        </span>
      </div>
    </InfoBanner>

    <template v-if="usingFile">
      <p class="mt-5 text-[13px] font-semibold">Site name</p>
      <p class="mt-2 font-mono text-[13px]" data-testid="project-file-name">
        {{ fileName }}.{{ tld }}
      </p>
      <p class="mt-1 text-xs text-muted-foreground">
        PHP, HTTPS, Node, domains and services come from the file.
      </p>
    </template>

    <template v-else>
      <label class="mt-5 block text-[13px] font-semibold" for="link-name">Site name</label>
      <SuffixInput
        id="link-name"
        class="mt-2"
        :model-value="name"
        :suffix="`.${tld}`"
        placeholder="myapp"
        aria-label="Site name"
        @update:model-value="typeName"
        @keydown.enter="valid && submit()"
      />

      <p v-if="ignoringFile" class="mt-5 text-xs text-muted-foreground" data-testid="project-file-defaults">
        The site is linked with the default PHP version and HTTPS off. Changing
        either later also updates <code class="font-mono">.yerd.toml</code>.
      </p>
      <div v-else class="mt-5 grid grid-cols-[auto_1fr] items-start gap-x-10 gap-y-2">
        <span class="text-[13px] font-semibold">PHP version</span>
        <span class="text-[13px] font-semibold">HTTPS</span>
        <div>
          <SegmentedControl
            v-if="phpOptions.length && phpOptions.length <= SEGMENTED_MAX"
            v-model="php"
            :options="phpOptions"
            aria-label="PHP version"
          />
          <Select
            v-else-if="phpOptions.length"
            v-model="php"
            :options="phpSelectOptions"
            aria-label="PHP version"
          />
          <span v-else class="text-xs text-muted-foreground">No PHP installed.</span>
        </div>
        <div class="flex h-7 items-center gap-2.5">
          <Switch v-model="secure" aria-label="Serve over HTTPS" />
          <span class="text-[13px] text-muted-foreground">{{ secure ? "On" : "Off" }}</span>
        </div>
      </div>
    </template>

    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Cancel</Button>
      <Button :disabled="!valid || looking" @click="submit"><Link2 /> Link site</Button>
    </template>
  </DialogFrame>
</template>
