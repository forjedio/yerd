<script setup lang="ts">
import { computed, ref, watch } from "vue";

import ServiceIcon from "@/components/services/ServiceIcon.vue";
import Button from "@/components/ui/Button.vue";
import DialogFrame from "@/components/ui/DialogFrame.vue";
import Select from "@/components/ui/Select.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { useToast } from "@/composables/useToast";
import { addableServiceTypes, IpcError, listSites } from "@/ipc/client";
import type { AddableServiceType, ServiceStatus, SiteEntry } from "@/ipc/types";
import { cn } from "@/lib/utils";

/** The `addService` argument object the dialog submits. */
export interface AddServiceRequest {
  type_id: string;
  site: string | null;
  port: number;
  version: string | null;
  autostart: boolean;
}

/**
 * The one-step Add a service dialog: a tile per service type (installed engines
 * read "Added" and can't be picked), then the version, Laravel site, port and
 * Start with Yerd for the picked type. The add is one blocking daemon call with
 * nothing to report until it finishes, so while `adding` the dialog stays open
 * with every control disabled and the view closes it when the call settles.
 */
const props = defineProps<{
  open: boolean;
  adding: boolean;
  /** The configured services, to skip sites that already host a per-site type. */
  services: ServiceStatus[];
  tld: string;
}>();

const emit = defineEmits<{
  "update:open": [boolean];
  add: [request: AddServiceRequest, displayName: string];
}>();

const toast = useToast();

const loading = ref(false);
const types = ref<AddableServiceType[]>([]);
const laravelSites = ref<SiteEntry[]>([]);
const form = ref({ typeId: "", site: "", version: "", port: "", autostart: false });

const selectedType = computed(() =>
  types.value.find((t) => t.type_id === form.value.typeId && !t.already_installed),
);

/** Sites that already host the selected per-site type; a site can host one. */
const usedSites = computed(() => {
  const t = selectedType.value;
  if (!t) return new Set<string>();
  return new Set(
    props.services
      .filter((s) => (s.type_id ?? "") === t.type_id && s.site)
      .map((s) => s.site as string),
  );
});

const siteOptions = computed(() =>
  laravelSites.value
    .filter((s) => !usedSites.value.has(s.name))
    .map((s) => ({ value: s.name, label: `${s.name}.${props.tld}` })),
);

const versionOptions = computed(() =>
  (selectedType.value?.available_versions ?? []).map((v) => ({ value: v, label: v })),
);

function syncAddDefaults(): void {
  const t = selectedType.value;
  if (!t) return;
  form.value.port = String(t.suggested_port);
  form.value.autostart = !t.requires_site;
  const vers = t.available_versions;
  form.value.version = vers[vers.length - 1] ?? "";
  form.value.site = t.requires_site ? (siteOptions.value[0]?.value ?? "") : "";
}

watch(() => form.value.typeId, syncAddDefaults);

async function load(): Promise<void> {
  form.value = { typeId: "", site: "", version: "", port: "", autostart: false };
  loading.value = true;
  try {
    const [t, sites] = await Promise.all([addableServiceTypes(), listSites()]);
    types.value = t;
    laravelSites.value = sites.filter((s) => s.is_laravel);
  } catch (e) {
    toast.error("Couldn't load service types", (e as IpcError).message);
  } finally {
    loading.value = false;
  }
}

watch(
  () => props.open,
  (open) => {
    if (open) void load();
  },
  { immediate: true },
);

const portValid = computed(() => {
  const p = Number(form.value.port);
  return Number.isInteger(p) && p >= 1 && p <= 65535;
});

const canSubmitAdd = computed(() => {
  const t = selectedType.value;
  if (!t) return false;
  if (t.requires_site && !form.value.site) return false;
  if (t.requires_version && !form.value.version) return false;
  return portValid.value;
});

const nextFree = computed(
  () => !!selectedType.value && form.value.port === String(selectedType.value.suggested_port),
);

const subtitle = computed(() => {
  const t = selectedType.value;
  if (!t) return "";
  if (t.requires_site) {
    return form.value.site
      ? `Runs on ${form.value.site}.${props.tld}'s PHP — nothing to download`
      : "Runs on the site's PHP — nothing to download";
  }
  if (t.type_id === "versitygw") {
    const what = form.value.version ? `versitygw ${form.value.version}` : "versitygw";
    return form.value.autostart
      ? `Downloads ${what}, generates access keys for this install, and starts it now and whenever Yerd starts.`
      : `Downloads ${what}, generates access keys for this install, and starts it now.`;
  }
  return form.value.autostart
    ? "Downloads a prebuilt build and starts it now and whenever Yerd starts."
    : "Downloads a prebuilt build and starts it now.";
});

const submitLabel = computed(() => {
  const t = selectedType.value;
  if (!t) return "Add";
  return t.type_id === "versitygw" ? "Add S3 storage" : `Add ${t.display_name}`;
});

function tileNote(t: AddableServiceType): string {
  if (t.already_installed) return "Added";
  if (t.requires_site) return "One per Laravel site";
  if (t.type_id === "versitygw") return "Object storage";
  return "Available";
}

function pick(t: AddableServiceType): void {
  if (t.already_installed || props.adding) return;
  form.value.typeId = t.type_id;
}

function onOpen(open: boolean): void {
  if (!props.adding) emit("update:open", open);
}

function submit(): void {
  const t = selectedType.value;
  if (!t || !canSubmitAdd.value || props.adding) return;
  const request: AddServiceRequest = {
    type_id: t.type_id,
    site: t.requires_site ? form.value.site : null,
    port: Number(form.value.port),
    version: t.requires_version ? form.value.version : null,
    autostart: form.value.autostart,
  };
  emit("add", request, t.display_name);
}

const LABEL = "mb-1.5 block text-xs font-medium";
</script>

<template>
  <DialogFrame :open="open" title="Add a service" width="lg" @update:open="onOpen">
    <div v-if="loading" class="flex justify-center py-10"><Spinner class="size-5" /></div>

    <p v-else-if="!types.length" class="py-8 text-center text-sm text-muted-foreground">
      No services are available to add.
    </p>

    <div v-else>
      <div class="grid grid-cols-3 gap-2" role="group" aria-label="Service type">
        <button
          v-for="t in types"
          :key="t.type_id"
          type="button"
          :data-type="t.type_id"
          :aria-pressed="form.typeId === t.type_id"
          :disabled="t.already_installed || adding"
          :class="
            cn(
              'flex h-16 min-w-0 flex-col justify-center gap-1 rounded-lg border px-3 text-left transition-colors focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:cursor-not-allowed',
              form.typeId === t.type_id
                ? 'border-brand/50 bg-brand/[0.14]'
                : 'bg-background hover:bg-accent/60 disabled:hover:bg-background',
              t.already_installed && 'opacity-70',
            )
          "
          @click="pick(t)"
        >
          <span class="flex min-w-0 items-center gap-2 text-[13px] font-semibold">
            <ServiceIcon :type-id="t.type_id" class="size-3.5 shrink-0 text-muted-foreground" />
            <span class="truncate">{{ t.display_name }}</span>
          </span>
          <span
            :class="cn('truncate text-[11px]', t.already_installed ? 'text-faint' : 'text-muted-foreground')"
          >
            {{ tileNote(t) }}
          </span>
        </button>
      </div>

      <p v-if="!selectedType" class="mt-5 text-[13px] text-faint">Pick a service to add.</p>

      <div v-else class="mt-5 space-y-4" data-testid="add-fields">
        <div class="flex gap-3">
          <div v-if="selectedType.requires_site" class="min-w-0 flex-1">
            <label :class="LABEL">Laravel site</label>
            <Select
              v-if="siteOptions.length"
              v-model="form.site"
              :options="siteOptions"
              :disabled="adding"
              class="w-full font-mono"
              aria-label="Laravel site"
            />
            <p v-else-if="laravelSites.length" class="text-xs italic text-muted-foreground">
              Every Laravel site already has a {{ selectedType.display_name }} instance.
            </p>
            <p v-else class="text-xs italic text-muted-foreground">
              No Laravel sites found. Park or link a site with an <code>artisan</code> file first.
            </p>
          </div>
          <div v-else-if="selectedType.requires_version" class="min-w-0 flex-1">
            <label :class="LABEL">Version</label>
            <Select
              v-model="form.version"
              :options="versionOptions"
              :disabled="adding"
              class="w-full font-mono"
              aria-label="Version"
            />
          </div>
          <div class="w-40 shrink-0">
            <label :class="LABEL" for="add-service-port">Port</label>
            <div
              class="flex h-9 items-center rounded-md border border-input bg-background px-3 focus-within:border-brand/60 focus-within:ring-1 focus-within:ring-brand/40"
            >
              <input
                id="add-service-port"
                v-model="form.port"
                inputmode="numeric"
                class="min-w-0 flex-1 bg-transparent font-mono text-sm outline-none disabled:cursor-not-allowed"
                aria-label="Port"
                :disabled="adding"
              />
              <span v-if="nextFree" class="shrink-0 text-[11px] text-faint" data-testid="next-free">next free</span>
            </div>
          </div>
        </div>

        <div class="flex items-center justify-between gap-4 rounded-lg border bg-background px-3.5 py-2.5">
          <div class="min-w-0">
            <p class="text-[13px] font-semibold">Start with Yerd</p>
            <p class="truncate text-[11px] text-muted-foreground" data-testid="add-subtitle">{{ subtitle }}</p>
          </div>
          <Switch v-model="form.autostart" :disabled="adding" aria-label="Start with Yerd" />
        </div>
      </div>
    </div>

    <template #footer-start>
      <template v-if="adding">Downloading and starting - this can take a few minutes.</template>
    </template>
    <template #footer>
      <Button variant="outline" :disabled="adding" @click="onOpen(false)">Cancel</Button>
      <Button :disabled="!canSubmitAdd || adding" data-testid="add-submit" @click="submit">
        <Spinner v-if="adding" class="size-4" />
        <template v-if="adding">Installing…</template>
        <template v-else>{{ submitLabel }}</template>
      </Button>
    </template>
  </DialogFrame>
</template>
