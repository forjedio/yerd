<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Download } from "lucide-vue-next";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import InfoBanner from "@/components/ui/InfoBanner.vue";
import Input from "@/components/ui/Input.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { useResource } from "@/composables/useResource";
import { availableNode } from "@/ipc/client";
import type { MajorLine } from "@/ipc/types";
import { ltsLabel } from "@/lib/node";

/**
 * Install a Node version: a release line not yet installed (end-of-life lines
 * behind a toggle) or an exact version. Opening it refetches the release index
 * (the view shares the same cache entry), so the picker offers each line's
 * newest patch. The install itself runs in the view.
 */
const props = defineProps<{
  open: boolean;
  /** An install or another Node mutation is running. */
  locked: boolean;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  install: [version: string];
}>();

const {
  data: majorsData,
  loading: majorsLoading,
  error: majorsError,
  refresh: reloadMajors,
} = useResource("node-available", availableNode, { immediate: false });

const majors = computed<MajorLine[]>(() => majorsData.value ?? []);
const eolKnown = computed(() => majors.value.some((l) => typeof l.eol === "boolean"));

function lineLabel(l: MajorLine): string {
  const parts = [`Node ${l.major} (${l.latest})`];
  if (l.codename) parts.push(ltsLabel(l.codename));
  if (l.eol === true) parts.push("EOL");
  return parts.join(" - ");
}

/** Release lines with nothing installed yet, newest major first, as the daemon orders them. */
const uninstalledLines = computed(() => majors.value.filter((l) => !l.installed_latest));

/** End-of-life lines are hidden from the picker unless asked for. */
const showEol = ref(false);
const hasEolLines = computed(() => uninstalledLines.value.some((l) => l.eol === true));

const installOptions = computed(() =>
  uninstalledLines.value
    .filter((l) => showEol.value || l.eol !== true)
    .map((l) => ({ value: String(l.major), label: lineLabel(l) })),
);

/** The newest supported LTS line not yet installed, else the newest line on offer. */
function preferredMajor(): string {
  const lts = majors.value.find((l) => !l.installed_latest && l.codename && l.eol !== true);
  return lts ? String(lts.major) : (installOptions.value[0]?.value ?? "");
}

const selectedMajor = ref("");
const exactMode = ref(false);
const exactVersion = ref("");

const canInstall = computed(() =>
  exactMode.value ? exactVersion.value.trim() !== "" : selectedMajor.value !== "",
);

/** Keep the pick valid as the list loads or changes, without overriding a still-offered choice. */
watch(installOptions, (options) => {
  if (!options.some((o) => o.value === selectedMajor.value)) {
    selectedMajor.value = preferredMajor();
  }
});

/** Reset the form and refetch the index. Exact mode starts on only when no line is left to pick. */
async function reset(): Promise<void> {
  exactMode.value = false;
  exactVersion.value = "";
  showEol.value = false;
  selectedMajor.value = preferredMajor();
  await reloadMajors({ force: true });
  if (!uninstalledLines.value.length && !majorsError.value) exactMode.value = true;
}

watch(
  () => props.open,
  (isOpen) => {
    if (isOpen) void reset();
  },
);

function submit(): void {
  const v = exactMode.value ? exactVersion.value.trim() : selectedMajor.value;
  if (!v || props.locked) return;
  emit("install", v);
  emit("update:open", false);
}
</script>

<template>
  <DialogFrame
    :open="open"
    title="Install a Node version"
    width="md"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <div class="space-y-4" data-testid="node-install-body">
      <InfoBanner v-if="majorsError" variant="destructive" data-testid="node-releases-error">
        <div class="flex flex-wrap items-center gap-2">
          <span>Couldn't load the Node release list: {{ majorsError.message }}</span>
          <Button variant="outline" size="sm" @click="reloadMajors({ force: true })">
            Retry
          </Button>
        </div>
      </InfoBanner>

      <div class="space-y-2.5">
        <div class="flex items-center gap-2.5 text-[13px]">
          <Switch
            v-model="exactMode"
            aria-labelledby="node-exact-mode-label"
            data-testid="node-exact-toggle"
          />
          <span id="node-exact-mode-label" class="cursor-pointer" @click="exactMode = !exactMode">
            Install an exact version
          </span>
        </div>

        <div v-if="!exactMode && hasEolLines" class="flex items-center gap-2.5 text-[13px]">
          <Switch
            v-model="showEol"
            aria-labelledby="node-eol-mode-label"
            data-testid="node-eol-toggle"
          />
          <span id="node-eol-mode-label" class="cursor-pointer" @click="showEol = !showEol">
            Show end-of-life versions
          </span>
        </div>
      </div>

      <div class="border-t pt-4">
        <span class="text-[13px] font-medium">Version</span>
        <div class="mt-2">
          <Input
            v-if="exactMode"
            v-model="exactVersion"
            placeholder="22.9.0"
            aria-label="Exact Node version"
            class="font-mono"
            @keydown.enter="submit"
          />
          <div v-else-if="majorsLoading && !majorsData" class="flex justify-center py-4">
            <Spinner class="size-5" />
          </div>
          <Select
            v-else-if="installOptions.length"
            v-model="selectedMajor"
            class="w-full"
            :options="installOptions"
            aria-label="Node release to install"
          />
          <p v-else class="text-[13px] text-muted-foreground" data-testid="node-install-none">
            <template v-if="hasEolLines">
              Every supported release line is already installed. Show end-of-life versions, or use
              an exact version to add an older patch.
            </template>
            <template v-else>
              Every listed release line is already installed. Use an exact version to add an older
              patch.
            </template>
          </p>
        </div>
        <p class="mt-2 text-xs text-faint">
          <template v-if="exactMode">
            Any release from major 9 up, for example
            <code class="font-mono font-semibold text-muted-foreground">22.9.0</code> or
            <code class="font-mono font-semibold text-muted-foreground">18</code>.
          </template>
          <template v-else>
            Installs the newest patch of the chosen line beside any you already have.
          </template>
        </p>
        <p
          v-if="!exactMode && majors.length && !eolKnown"
          class="mt-1 text-xs text-faint"
          data-testid="node-eol-unknown"
        >
          End-of-life status unavailable.
        </p>
      </div>
    </div>

    <template #footer>
      <Button variant="ghost" @click="emit('update:open', false)">Cancel</Button>
      <Button :disabled="!canInstall || locked" data-testid="node-install-submit" @click="submit">
        <Download aria-hidden="true" />
        Install
      </Button>
    </template>
  </DialogFrame>
</template>
