<script setup lang="ts">
import { computed, ref, watch } from "vue";
import { Ellipsis, FileText, Play, RotateCw, Square, Trash2, X } from "lucide-vue-next";

import ServiceConnectCard from "@/components/services/ServiceConnectCard.vue";
import ServiceDatabasesCard from "@/components/services/ServiceDatabasesCard.vue";
import ServiceOverridesCard from "@/components/services/ServiceOverridesCard.vue";
import Button from "@/components/ui/Button.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { CARD, INSPECTOR_TOOL } from "@/components/ui/styles";
import type { ServiceStatus, SiteEntry, StatusReport } from "@/ipc/types";
import {
  address,
  canStart,
  canStop,
  connectKind,
  defaultConnectDb,
  hasVersion,
  headerStatus,
  isPerSite,
  siteDomain,
  uninstallVersion,
  versionLabel,
} from "@/lib/services";
import { cn, runStateDotClass } from "@/lib/utils";

/** The view's database list for one SQL engine. */
export interface DatabaseListState {
  names: string[] | null;
  loading: boolean;
  error: string | null;
}

/**
 * The Services inspector: the selected service's header (status, Logs and the
 * Start / Restart / Stop that apply, the `···` menu), the facts strip (address,
 * version or site, Start with Yerd) and the cards for its kind: Databases,
 * Connect and Config overrides for a SQL engine, Connect and overrides for a
 * cache or search engine, and Reverb's two-column Connect block. In `panel`
 * mode it sits beside the list; in `drawer` mode it slides over the window
 * (760px, for the two columns), and Escape is left to an open menu inside it.
 */
const props = defineProps<{
  mode: "panel" | "drawer";
  shown: boolean;
  service: ServiceStatus | null;
  databases: DatabaseListState;
  report: StatusReport | null;
  sites: SiteEntry[];
  tld: string;
  /** The view's in-flight action key, e.g. `restart:postgres`. */
  busy: string | null;
}>();

const emit = defineEmits<{
  close: [];
  start: [];
  stop: [];
  restart: [];
  logs: [];
  editPort: [];
  changeVersion: [];
  toggleAutostart: [enabled: boolean];
  uninstall: [];
  remove: [];
  databasesChanged: [];
  openSite: [site: SiteEntry];
}>();

const drawer = computed(() => props.mode === "drawer");
const headerMenuOpen = ref(false);
const databasesLayerOpen = ref(false);
const connectDb = ref("");

function isLayerOpen(): boolean {
  return headerMenuOpen.value || databasesLayerOpen.value;
}

watch(
  [() => props.service?.service, () => props.databases.names],
  ([id, names], [prevId]) => {
    const list = names ?? [];
    if (id !== prevId || !list.includes(connectDb.value)) connectDb.value = defaultConnectDb(list);
  },
  { immediate: true },
);

const id = computed(() => props.service?.service ?? "");
const perSite = computed(() => !!props.service && isPerSite(props.service));
const rowBusy = computed(() => props.busy?.endsWith(`:${id.value}`) ?? false);
const state = computed(() => props.service?.state);
const linkedSite = computed(() => props.sites.find((s) => s.name === props.service?.site) ?? null);
const kind = computed(() => (props.service ? connectKind(props.service) : "none"));
const hasConnect = computed(() => kind.value !== "none");
const hasOverrides = computed(() => props.service?.supports_overrides === true);
const hasDatabases = computed(() => props.service?.supports_databases === true && !perSite.value);

function actionBusy(action: string): boolean {
  return props.busy === `${action}:${id.value}`;
}

const CELL = "flex min-w-0 flex-col justify-center gap-1 px-[18px] py-3";
const CELL_LABEL = "text-[11px] font-medium uppercase tracking-[0.6px] text-faint";
const LINK =
  "shrink-0 rounded text-xs font-medium text-brand hover:underline focus-visible:outline-none focus-visible:ring-1 focus-visible:ring-ring disabled:pointer-events-none disabled:text-faint";
</script>

<template>
  <InspectorFrame
    :mode="mode"
    :shown="shown"
    label="Service details"
    testid="service-inspector"
    backdrop-class="service-inspector-backdrop"
    wide-drawer
    :layer-open="isLayerOpen"
    @close="emit('close')"
  >
    <div v-if="service" :key="service.service" class="flex flex-1 flex-col px-7 pb-6 pt-6">
      <div class="flex items-start gap-3" data-testid="service-header">
        <div class="min-w-0 flex-1">
          <h2 class="flex items-baseline gap-2.5">
            <span class="truncate text-[22px] font-semibold leading-tight tracking-[-0.2px]">
              {{ service.display_name }}
            </span>
            <span v-if="hasVersion(service)" class="shrink-0 font-mono text-[15px] text-muted-foreground">
              {{ versionLabel(service) }}
            </span>
            <span v-else-if="service.site" class="truncate font-mono text-[15px] text-muted-foreground">
              {{ siteDomain(service, tld, sites) }}
            </span>
          </h2>
          <p class="mt-1.5 flex items-center gap-1.5 text-xs text-muted-foreground" data-testid="service-status">
            <span :class="cn('size-1.5 shrink-0 rounded-full', runStateDotClass(service.state))" aria-hidden="true" />
            {{ headerStatus(service) }}
          </p>
          <p
            v-if="service.state === 'failed' && service.error"
            class="mt-1 line-clamp-2 text-xs text-destructive"
            :title="service.error"
            data-testid="service-error"
          >
            {{ service.error }}
          </p>
        </div>

        <div class="flex shrink-0 items-center gap-2">
          <Button variant="outline" :class="INSPECTOR_TOOL" data-action="logs" @click="emit('logs')">
            <FileText /> Logs
          </Button>
          <Button
            v-if="canStart(service)"
            :variant="state === 'stopped' ? 'default' : 'outline'"
            :class="INSPECTOR_TOOL"
            :disabled="rowBusy"
            data-action="start"
            @click="emit('start')"
          >
            <Spinner v-if="actionBusy('start')" class="size-4" />
            <Play v-else />
            Start
          </Button>
          <Button
            v-if="state === 'running'"
            variant="outline"
            :class="INSPECTOR_TOOL"
            :disabled="rowBusy"
            data-action="restart"
            @click="emit('restart')"
          >
            <Spinner v-if="actionBusy('restart')" class="size-4" />
            <RotateCw v-else />
            Restart
          </Button>
          <Button
            v-if="canStop(service)"
            variant="outline"
            :class="INSPECTOR_TOOL"
            :disabled="rowBusy"
            data-action="stop"
            @click="emit('stop')"
          >
            <Spinner v-if="actionBusy('stop')" class="size-4" />
            <Square v-else />
            Stop
          </Button>
          <DropdownMenu v-model:open="headerMenuOpen" @vue:before-unmount="headerMenuOpen = false">
            <DropdownMenuTrigger as-child>
              <Button
                variant="outline"
                size="icon"
                class="size-[34px] shrink-0"
                :aria-label="`More actions for ${service.display_name}`"
              >
                <Ellipsis />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" class="w-[220px]">
              <DropdownMenuItem
                v-if="perSite"
                :disabled="rowBusy"
                class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                data-action="remove"
                @select="emit('remove')"
              >
                <Trash2 class="size-4" /> Remove this instance
              </DropdownMenuItem>
              <DropdownMenuItem
                v-else
                :disabled="rowBusy"
                class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                data-action="uninstall"
                @select="emit('uninstall')"
              >
                <Trash2 class="size-4" /> Uninstall {{ service.display_name }} {{ uninstallVersion(service) }}…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Button
            v-if="drawer"
            variant="outline"
            size="icon"
            class="size-[34px] shrink-0"
            aria-label="Close service details"
            @click="emit('close')"
          >
            <X />
          </Button>
        </div>
      </div>

      <div :class="cn(CARD, 'mt-5 grid min-h-[62px] grid-cols-3 divide-x')" data-testid="service-facts">
        <div :class="CELL">
          <span :class="CELL_LABEL">Address</span>
          <span class="flex items-center gap-2">
            <span class="min-w-0 flex-1 truncate font-mono text-[13px]">{{ address(service) }}</span>
            <button type="button" :class="LINK" :disabled="rowBusy" @click="emit('editPort')">Edit port</button>
          </span>
        </div>
        <div v-if="perSite" :class="CELL" data-testid="fact-site">
          <span :class="CELL_LABEL">Site</span>
          <span class="flex items-center gap-2">
            <span class="min-w-0 flex-1 truncate font-mono text-[13px]">{{ siteDomain(service, tld, sites) }}</span>
            <button
              type="button"
              :class="LINK"
              :disabled="!linkedSite"
              :title="linkedSite ? undefined : 'Site not found'"
              @click="linkedSite && emit('openSite', linkedSite)"
            >
              Open
            </button>
          </span>
        </div>
        <div v-else :class="CELL" data-testid="fact-version">
          <span :class="CELL_LABEL">Version</span>
          <span class="flex items-center gap-2">
            <span class="min-w-0 flex-1 truncate font-mono text-[13px]">{{ versionLabel(service) }}</span>
            <button type="button" :class="LINK" :disabled="rowBusy" @click="emit('changeVersion')">Change</button>
          </span>
        </div>
        <div :class="CELL">
          <span :class="CELL_LABEL">Start with Yerd</span>
          <span class="flex items-center gap-2">
            <span class="min-w-0 flex-1 text-[13px]">{{ service.enabled ? "On" : "Off" }}</span>
            <Switch
              :model-value="service.enabled"
              :disabled="rowBusy"
              :aria-label="`Start ${service.display_name} with Yerd`"
              @update:model-value="(v: boolean) => emit('toggleAutostart', v)"
            />
          </span>
        </div>
      </div>

      <div
        v-if="hasDatabases"
        class="mt-5 grid grid-cols-[minmax(0,1fr)_324px] items-start gap-4"
        data-testid="service-body"
      >
        <ServiceDatabasesCard
          v-model:layer-open="databasesLayerOpen"
          :service="service"
          :names="databases.names"
          :loading="databases.loading"
          :error="databases.error"
          :connect-db="connectDb"
          @use-in-connect="(name: string) => (connectDb = name)"
          @changed="emit('databasesChanged')"
        />
        <div class="flex min-w-0 flex-col gap-4">
          <ServiceConnectCard
            v-if="hasConnect"
            v-model:database="connectDb"
            :service="service"
            :databases="databases.names"
            :report="report"
            :sites="sites"
            :tld="tld"
            layout="stack"
          />
          <ServiceOverridesCard v-if="hasOverrides" :service="service" />
        </div>
      </div>

      <div v-else-if="perSite" class="mt-5" data-testid="service-body">
        <ServiceConnectCard
          :service="service"
          :databases="null"
          :report="report"
          :sites="sites"
          :tld="tld"
          layout="wide"
        />
      </div>

      <div
        v-else-if="hasConnect || hasOverrides"
        :class="cn('mt-5 grid items-start gap-4', hasConnect && hasOverrides ? 'grid-cols-2' : 'grid-cols-1')"
        data-testid="service-body"
      >
        <ServiceConnectCard
          v-if="hasConnect"
          :service="service"
          :databases="null"
          :report="report"
          :sites="sites"
          :tld="tld"
          layout="stack"
        />
        <ServiceOverridesCard v-if="hasOverrides" :service="service" />
      </div>
    </div>
  </InspectorFrame>
</template>
