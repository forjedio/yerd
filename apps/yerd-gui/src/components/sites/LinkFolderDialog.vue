<script setup lang="ts">
import { Folder, Link2 } from "lucide-vue-next";
import { computed, ref, watch } from "vue";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Select from "@/components/ui/Select.vue";
import SuffixInput from "@/components/ui/SuffixInput.vue";
import Switch from "@/components/ui/Switch.vue";
import { preferredPhp } from "@/composables/useSiteStarters";
import { pickDirectory } from "@/ipc/client";
import { comparePhpVersions } from "@/lib/phpVersion";
import { slugifySiteName } from "@/lib/siteName";
import { tildify } from "@/lib/sites";

/** What the Link dialog asks the Sites view to do. */
export interface LinkRequest {
  name: string;
  path: string;
  php: string;
  secure: boolean;
}

/**
 * "Link an existing folder": pick a project folder, name the site (prefilled
 * from the folder until the user types their own), and choose its PHP version
 * and HTTPS. Emits the request; the Sites view links it and applies the rest.
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

const path = ref("");
const name = ref("");
/** True once the user types in the name field, so choosing a folder never
 *  clobbers a name they entered themselves. */
const nameDirty = ref(false);
const php = ref("");
const secure = ref(false);

const versions = computed(() => [...props.phpVersions].sort(comparePhpVersions));
const phpOptions = computed(() => versions.value.map((v) => ({ value: v, label: v })));
const phpSelectOptions = computed(() => versions.value.map((v) => ({ value: v, label: `PHP ${v}` })));

/** A folder-derived or typed name is converted to a valid slug on submit, so
 *  validity is "does it slugify to something", not the raw `[a-z0-9-]` shape. */
const valid = computed(() => slugifySiteName(name.value) !== null && path.value.trim() !== "");

watch(
  () => props.open,
  (open) => {
    if (open) {
      php.value = preferredPhp(props.phpVersions, props.defaultPhp, () => true);
      return;
    }
    path.value = "";
    name.value = "";
    nameDirty.value = false;
    secure.value = false;
  },
  { immediate: true },
);

function baseName(p: string): string {
  const trimmed = p.replace(/[/\\]+$/, "");
  const parts = trimmed.split(/[/\\]/);
  return parts[parts.length - 1] ?? "";
}

async function chooseFolder(): Promise<void> {
  const dir = await pickDirectory(path.value || undefined);
  if (!dir) return;
  path.value = dir;
  if (!nameDirty.value) name.value = slugifySiteName(baseName(dir)) ?? "";
}

function typeName(value: string): void {
  nameDirty.value = true;
  name.value = value;
}

function submit(): void {
  const slug = slugifySiteName(name.value);
  if (!slug || !path.value.trim()) return;
  emit("link", { name: slug, path: path.value.trim(), php: php.value, secure: secure.value });
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
      <div class="min-w-0 flex-1">
        <p v-if="path" class="truncate font-mono text-[13px]" data-testid="link-path">
          {{ tildify(path, home) }}
        </p>
        <p v-else class="text-[13px] text-muted-foreground">Pick the project folder to serve</p>
      </div>
      <Button variant="outline" size="sm" @click="chooseFolder">
        {{ path ? "Change" : "Choose folder…" }}
      </Button>
    </div>

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

    <div class="mt-5 grid grid-cols-[auto_1fr] items-start gap-x-10 gap-y-2">
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

    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Cancel</Button>
      <Button :disabled="!valid" @click="submit"><Link2 /> Link site</Button>
    </template>
  </DialogFrame>
</template>
