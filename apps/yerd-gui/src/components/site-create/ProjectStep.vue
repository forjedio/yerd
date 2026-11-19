<script setup lang="ts">
import {
  ChevronsUpDown,
  CircleCheck,
  Feather,
  Folder,
  FolderInput,
  Lock,
  LockOpen,
  Package,
  Rocket,
} from "lucide-vue-next";
import { computed, ref, type Component } from "vue";

import StarterPicker from "@/components/site-create/StarterPicker.vue";
import SegmentedControl from "@/components/ui/SegmentedControl.vue";
import Select from "@/components/ui/Select.vue";
import SuffixInput from "@/components/ui/SuffixInput.vue";
import Switch from "@/components/ui/Switch.vue";
import type { ActiveStarter, StarterId } from "@/composables/useSiteStarters";
import { pickDirectory } from "@/ipc/client";
import { comparePhpVersions } from "@/lib/phpVersion";
import { tildify } from "@/lib/sites";
import { cn } from "@/lib/utils";

/**
 * Step 1 of New site: what to start from, the project's name, where it goes, its
 * PHP version and HTTPS. The form lives in the dialog; this step only binds it.
 */
const props = defineProps<{
  /** The starter currently picked, for its PHP window and labels. */
  model: ActiveStarter;
  parkedFolders: string[];
  phpVersions: string[];
  tld: string;
  home: string | null;
  openUrl: string;
  projectPath: string;
  nameValid: boolean;
  caTrusted: boolean;
}>();

const emit = defineEmits<{
  existing: [];
  submit: [];
}>();

const starter = defineModel<StarterId>("starter", { required: true });
const name = defineModel<string>("name", { required: true });
const location = defineModel<string>("location", { required: true });
const php = defineModel<string>("php", { required: true });
const secure = defineModel<boolean>("secure", { required: true });

const TILES: { id: StarterId; label: string; hint: string; icon: Component; tone: string }[] = [
  { id: "laravel", label: "Laravel", hint: "Kits, auth, Pest", icon: Rocket, tone: "bg-red-500/15 text-red-500" },
  {
    id: "wordpress",
    label: "WordPress",
    hint: "Latest release, admin ready",
    icon: Package,
    tone: "bg-cyan-500/15 text-cyan-500",
  },
  { id: "slim", label: "Slim", hint: "Micro framework", icon: Feather, tone: "bg-green-500/15 text-green-500" },
];

/** Above this many versions the segmented control would overflow its column. */
const SEGMENTED_MAX = 6;

/** The Location select's last option, which opens the folder picker. */
const CHOOSE = "__choose__";

const supported = computed(() =>
  props.phpVersions.filter((v) => props.model.supportsPhp(v)).sort(comparePhpVersions),
);
const phpOptions = computed(() =>
  supported.value.map((v) => ({ value: v, label: props.model.phpLabel(v) })),
);
const phpSelectOptions = computed(() =>
  supported.value.map((v) => ({ value: v, label: `PHP ${props.model.phpLabel(v)}` })),
);

const parked = computed(() => !!location.value && props.parkedFolders.includes(location.value));

/** Parked roots, a custom-picked folder that isn't one of them, then the picker entry. */
const locationOptions = computed(() => {
  const opts = props.parkedFolders.map((f) => ({ value: f, label: tildify(f, props.home) }));
  if (location.value && !props.parkedFolders.includes(location.value)) {
    opts.unshift({ value: location.value, label: tildify(location.value, props.home) });
  }
  return [...opts, { value: CHOOSE, label: "Choose another folder…" }];
});

const locationKey = ref(0);

async function changeLocation(value: string): Promise<void> {
  if (value !== CHOOSE) {
    location.value = value;
    return;
  }
  const dir = await pickDirectory(location.value || undefined);
  if (dir) location.value = dir;
  locationKey.value += 1;
}
</script>

<template>
  <div>
    <h3 class="text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">Start from</h3>
    <div class="mt-2.5 grid grid-cols-3 gap-2.5">
      <button
        v-for="t in TILES"
        :key="t.id"
        type="button"
        :data-starter="t.id"
        :aria-pressed="starter === t.id"
        :class="
          cn(
            'flex h-[58px] min-w-0 items-center gap-2.5 rounded-[10px] border px-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring',
            starter === t.id ? 'border-brand bg-brand/10 ring-1 ring-brand' : 'bg-background hover:border-brand/40',
          )
        "
        @click="starter = t.id"
      >
        <span :class="cn('flex size-8 shrink-0 items-center justify-center rounded-lg', t.tone)">
          <component :is="t.icon" class="size-4" aria-hidden="true" />
        </span>
        <span class="min-w-0">
          <span class="flex items-center gap-1.5 text-sm font-semibold">
            {{ t.label }}
            <CircleCheck v-if="starter === t.id" class="size-3.5 text-brand" aria-hidden="true" />
          </span>
          <span class="block truncate text-[11px] text-muted-foreground">{{ t.hint }}</span>
        </span>
      </button>
    </div>
    <div class="mt-2.5 grid grid-cols-3 gap-2.5">
      <div class="col-span-2 min-w-0">
        <StarterPicker v-model="starter" />
      </div>
      <button
        type="button"
        class="flex h-11 items-center justify-center gap-2 rounded-[10px] border border-dashed px-3 text-[13px] font-semibold text-foreground transition-colors hover:border-brand/50 hover:bg-accent/50 focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring"
        @click="emit('existing')"
      >
        <FolderInput class="size-4 text-muted-foreground" aria-hidden="true" /> Existing folder
      </button>
    </div>

    <label class="mt-5 block text-[13px] font-semibold" for="ns-name">Project name</label>
    <SuffixInput
      id="ns-name"
      v-model="name"
      class="mt-2"
      :suffix="`.${tld}`"
      placeholder="invoicer"
      aria-label="Project name"
      :invalid="!!name && !nameValid"
      @keydown.enter="emit('submit')"
    />
    <p v-if="name && !nameValid" class="mt-1.5 text-xs text-destructive">
      Use a single label: letters, numbers and hyphens only.
    </p>
    <p v-else class="mt-1.5 flex min-w-0 items-center gap-1.5 text-[11px] text-faint" data-testid="ns-preview">
      <Lock v-if="secure" class="size-3 shrink-0 text-success" aria-hidden="true" />
      <LockOpen v-else class="size-3 shrink-0" aria-hidden="true" />
      <span class="truncate font-mono text-muted-foreground">{{ openUrl }}</span>
      <span v-if="projectPath" class="truncate">· created in {{ tildify(projectPath, home) }}</span>
    </p>

    <label class="mt-5 block text-[13px] font-semibold" for="ns-location">Location</label>
    <div
      class="mt-2 flex h-10 items-center gap-2.5 rounded-lg border bg-background px-3 focus-within:ring-1 focus-within:ring-ring"
    >
      <Folder class="size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
      <select
        id="ns-location"
        :key="locationKey"
        :value="location"
        aria-label="Location"
        class="min-w-0 flex-1 cursor-pointer appearance-none bg-transparent font-mono text-[13px] outline-none"
        @change="changeLocation(($event.target as HTMLSelectElement).value)"
      >
        <option v-if="!location" value="" disabled>Choose a folder…</option>
        <option v-for="o in locationOptions" :key="o.value" :value="o.value">{{ o.label }}</option>
      </select>
      <span
        v-if="location"
        class="shrink-0 rounded-md border bg-muted px-2 py-0.5 text-[11px] text-muted-foreground"
        data-testid="location-badge"
      >
        {{ parked ? "Parked · served automatically" : "Linked after creation" }}
      </span>
      <ChevronsUpDown class="pointer-events-none size-3.5 shrink-0 text-faint" aria-hidden="true" />
    </div>

    <div class="mt-5 grid grid-cols-2 gap-6">
      <div class="min-w-0">
        <p class="text-[13px] font-semibold" :title="model.phpHint(php)">PHP version</p>
        <div class="mt-2" :title="model.phpHint(php)">
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
          <p v-else class="text-xs" data-testid="ns-no-php">
            <span class="text-destructive">{{ model.noPhpText }}</span>
            <span class="text-muted-foreground"> Yerd installs it in the next step.</span>
          </p>
        </div>
      </div>
      <div class="min-w-0">
        <p class="text-[13px] font-semibold">HTTPS</p>
        <div class="mt-2 flex h-7 items-center gap-2.5">
          <Switch v-model="secure" aria-label="Serve over HTTPS" />
          <span class="truncate text-[13px] text-muted-foreground">
            {{ caTrusted ? "Trusted by Yerd local CA" : "Browsers will warn until the CA is trusted" }}
          </span>
        </div>
      </div>
    </div>
  </div>
</template>
