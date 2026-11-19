<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Check, Download, TriangleAlert } from "lucide-vue-next";
import { RadioGroupItem, RadioGroupRoot } from "reka-ui";

import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useToast } from "@/composables/useToast";
import { availablePhp, IpcError } from "@/ipc/client";
import type { PhpVersion } from "@/ipc/types";
import { type InstallChoice, installChoices } from "@/lib/php";
import { cn } from "@/lib/utils";

/** What the dialog asks the view to install. */
export interface InstallRequest {
  version: PhpVersion;
  legacy: boolean;
}

/**
 * Install PHP: every installable version in one radio list, supported first
 * and legacy (out of support) below. Installed rows are shown but disabled.
 * Picking a legacy row shows the warning and an acknowledgement that resets on
 * every new pick. The install itself runs in the view.
 */
const props = defineProps<{
  open: boolean;
  /** Whether a PHP install is already running (only one runs at a time). */
  installing: boolean;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  install: [request: InstallRequest];
}>();

const toast = useToast();
const loading = ref(false);
const supported = ref<InstallChoice[]>([]);
const legacy = ref<InstallChoice[]>([]);
const selected = ref<PhpVersion>("");
const acknowledged = ref(false);

const legacyPicked = computed(() => legacy.value.some((c) => c.version === selected.value));
const hasChoices = computed(() => supported.value.length > 0 || legacy.value.length > 0);
const anyInstallable = computed(() =>
  [...supported.value, ...legacy.value].some((c) => !c.installed),
);
const canInstall = computed(
  () => !!selected.value && !props.installing && (!legacyPicked.value || acknowledged.value),
);

watch(selected, () => {
  acknowledged.value = false;
});

async function load(): Promise<void> {
  loading.value = true;
  supported.value = [];
  legacy.value = [];
  selected.value = "";
  acknowledged.value = false;
  try {
    const r = await availablePhp();
    const c = installChoices(r.available, r.legacy ?? [], r.installed);
    supported.value = c.supported;
    legacy.value = c.legacy;
    selected.value = c.defaultChoice;
  } catch (e) {
    toast.error("Couldn't load installable versions", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

watch(
  () => props.open,
  (isOpen) => {
    if (isOpen) void load();
  },
  { immediate: true },
);

function onPick(value: unknown): void {
  if (typeof value === "string") selected.value = value;
}

function submit(): void {
  if (!canInstall.value) return;
  emit("install", { version: selected.value, legacy: legacyPicked.value });
  emit("update:open", false);
}

const groups = computed(() =>
  [
    { key: "supported", label: "Supported", note: "", rows: supported.value },
    {
      key: "legacy",
      label: "Legacy",
      note: "out of support — for maintaining old projects",
      rows: legacy.value,
    },
  ].filter((g) => g.rows.length > 0),
);
</script>

<template>
  <DialogFrame
    :open="open"
    title="Install PHP"
    width="lg"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <div v-if="loading" class="flex justify-center py-8"><Spinner class="size-5" /></div>

    <template v-else-if="hasChoices && anyInstallable">
      <RadioGroupRoot
        :model-value="selected"
        aria-label="PHP version to install"
        class="flex flex-col gap-4"
        @update:model-value="onPick"
      >
        <section v-for="g in groups" :key="g.key" :data-group="g.key">
          <p class="mb-2 flex items-baseline gap-2 px-1">
            <span class="text-[11px] font-semibold uppercase tracking-[0.6px] text-faint">{{ g.label }}</span>
            <span v-if="g.note" class="text-[11px] text-faint">{{ g.note }}</span>
          </p>
          <div class="divide-y overflow-hidden rounded-[10px] border">
            <RadioGroupItem
              v-for="c in g.rows"
              :key="c.version"
              :value="c.version"
              :disabled="c.installed"
              :data-version="c.version"
              :class="
                cn(
                  'flex h-[51px] w-full items-center gap-3 px-4 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-ring',
                  c.version === selected ? 'bg-brand/[0.14]' : 'hover:bg-accent/60',
                  c.installed && 'cursor-not-allowed opacity-50',
                )
              "
            >
              <span
                :class="
                  cn(
                    'flex size-4 shrink-0 items-center justify-center rounded-full border',
                    c.version === selected ? 'border-brand' : 'border-input',
                  )
                "
                aria-hidden="true"
              >
                <span v-if="c.version === selected" class="size-2 rounded-full bg-brand" />
              </span>
              <span class="flex-1 font-mono text-[13px] font-semibold">PHP {{ c.version }}</span>
              <span v-if="c.installed" class="flex items-center gap-1 text-xs text-faint">
                <Check class="size-3.5" /> Installed
              </span>
              <span
                v-else
                :class="cn('text-xs', c.version === selected ? 'text-brand' : 'text-muted-foreground')"
              >
                Available
              </span>
            </RadioGroupItem>
          </div>
        </section>
      </RadioGroupRoot>

      <div
        v-if="legacyPicked"
        class="mt-4 rounded-[10px] border border-warning/40 bg-warning/10 p-4"
        data-testid="legacy-warning"
      >
        <p class="flex items-center gap-2 text-[13px] font-semibold">
          <TriangleAlert class="size-4 shrink-0 text-warning" />
          PHP {{ selected }} is out of support
        </p>
        <p class="mt-1.5 text-xs text-muted-foreground">
          It may have unpatched security holes. It can't be the default, and has no
          coverage or dump capture. Use it only to maintain old projects.
        </p>
        <label
          class="mt-3 flex cursor-pointer items-center gap-2.5 rounded-md border border-warning/30 bg-background/40 px-3 py-2 text-[13px] font-medium"
        >
          <input
            v-model="acknowledged"
            type="checkbox"
            class="size-4 accent-[hsl(var(--warning))]"
            aria-label="I understand — install it anyway"
            data-testid="legacy-ack"
          />
          I understand — install it anyway
        </label>
      </div>
    </template>

    <p v-else class="py-2 text-sm text-muted-foreground" data-testid="install-empty">
      No installable versions to add - every version offered is already installed, or the list
      couldn't be reached.
    </p>

    <template #footer-start>Prebuilt static build · takes a few minutes</template>
    <template #footer>
      <Button variant="outline" @click="emit('update:open', false)">Cancel</Button>
      <Button :disabled="!canInstall" data-testid="install-submit" @click="submit">
        <Download />
        {{ selected ? `Install ${selected}` : "Install" }}
      </Button>
    </template>
  </DialogFrame>
</template>
