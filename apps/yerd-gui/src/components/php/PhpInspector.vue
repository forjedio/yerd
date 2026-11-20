<script setup lang="ts">
import { computed, ref } from "vue";
import { Download, Ellipsis, RotateCw, Star, Trash2, X } from "lucide-vue-next";

import PhpVersionPanel from "@/components/PhpVersionPanel.vue";
import PhpSharedDefaults from "@/components/php/PhpSharedDefaults.vue";
import type { PhpSelection } from "@/components/php/PhpVersionsList.vue";
import Button from "@/components/ui/Button.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { INSPECTOR_TOOL } from "@/components/ui/styles";
import type { PhpExtensionsMap } from "@/ipc/client";
import type { PhpExtInfo, PhpPoolStatus, PhpUpdate, PhpVersion, PhpVersionsResponse } from "@/ipc/types";
import { statusLine, updateLabel } from "@/lib/php";
import { isLegacyVersion } from "@/lib/phpVersion";
import { CHIP_TONE, cn, runStateDotClass } from "@/lib/utils";

/**
 * The PHP inspector: the Shared defaults pane or the selected version's header
 * (status, Restart pool, the `···` menu) over its configuration panel. Every
 * version's panel stays mounted (shown with `v-show`) so staged edits survive a
 * selection change. In `panel` mode it sits beside the list; in `drawer` mode it
 * slides over the window (see `InspectorFrame`), and Escape is left to an open
 * menu inside it.
 */
const props = defineProps<{
  mode: "panel" | "drawer";
  shown: boolean;
  selected: PhpSelection;
  versions: PhpVersion[];
  installed: PhpVersion[];
  defaultVersion: PhpVersion | null;
  data: PhpVersionsResponse | null;
  extensions: Record<PhpVersion, PhpExtInfo[]> | null;
  extensionsLoading: boolean;
  pools: Record<PhpVersion, PhpPoolStatus>;
  updates: PhpUpdate[];
  /** The view's in-flight action key, e.g. `restart:8.4`. */
  busy: string | null;
  anyRunning: boolean;
}>();

const emit = defineEmits<{
  close: [];
  restart: [version: PhpVersion];
  update: [version: PhpVersion];
  makeDefault: [version: PhpVersion];
  uninstall: [version: PhpVersion];
  restartAll: [];
  updated: [r: PhpVersionsResponse];
  extensionsUpdated: [map: PhpExtensionsMap];
  requestAddExtension: [version: PhpVersion];
  dirty: [version: PhpVersion, isDirty: boolean];
  sharedDirty: [isDirty: boolean];
}>();

const drawer = computed(() => props.mode === "drawer");
const version = computed<PhpVersion | null>(() => (props.selected === "shared" ? null : props.selected));
const isInstalled = computed(() => !!version.value && props.installed.includes(version.value));
const legacy = computed(() => !!version.value && isLegacyVersion(version.value));
const pool = computed(() => (version.value ? props.pools[version.value] : undefined));
const update = computed(() => props.updates.find((u) => u.version === version.value));
const canRestart = computed(() => pool.value?.state === "running" || pool.value?.state === "failed");

const headerMenuOpen = ref(false);
const menuOpenByVersion = ref<Record<string, boolean>>({});

function isLayerOpen(): boolean {
  return headerMenuOpen.value || Object.values(menuOpenByVersion.value).some(Boolean);
}

const dotClass = computed(() => runStateDotClass(pool.value?.state));

const makeDefaultTitle = computed(() => {
  if (version.value === props.defaultVersion) return "Already the default";
  if (legacy.value) return "Legacy versions can't be the default";
  return undefined;
});

function extensionsFor(v: PhpVersion): PhpExtInfo[] {
  return props.extensions?.[v] ?? [];
}

const CHIP = "inline-flex h-5 shrink-0 items-center gap-1 rounded px-1.5 text-[11px] font-medium";
</script>

<template>
  <InspectorFrame
    :mode="mode"
    :shown="shown"
    label="PHP details"
    testid="php-inspector"
    backdrop-class="php-inspector-backdrop"
    :layer-open="isLayerOpen"
    @close="emit('close')"
  >
    <div class="flex flex-1 flex-col px-7 pb-6 pt-6">
      <PhpSharedDefaults
        v-show="selected === 'shared'"
        :active="selected === 'shared'"
        :data="data"
        :restarting="busy === 'restart:all'"
        :any-running="anyRunning"
        @updated="(r) => emit('updated', r)"
        @restart-all="emit('restartAll')"
        @dirty="(d) => emit('sharedDirty', d)"
      >
        <template #header-extra>
          <Button
            v-if="drawer"
            variant="outline"
            size="icon"
            class="size-[34px] shrink-0"
            aria-label="Close PHP details"
            @click="emit('close')"
          >
            <X />
          </Button>
        </template>
      </PhpSharedDefaults>

      <div v-if="version" class="flex items-start gap-3" data-testid="version-header">
        <div class="min-w-0 flex-1">
          <h2 class="flex items-center gap-2.5">
            <span class="truncate font-mono text-[22px] font-semibold leading-tight tracking-[-0.2px]">
              PHP {{ version }}
            </span>
            <span v-if="version === defaultVersion" :class="cn(CHIP, CHIP_TONE.brand)">
              <Star class="size-3" aria-hidden="true" /> Default
            </span>
            <span v-if="legacy" :class="cn(CHIP, CHIP_TONE.warning)">Legacy</span>
          </h2>
          <p class="mt-1.5 flex items-center gap-1.5 text-xs text-faint" data-testid="version-status">
            <template v-if="isInstalled">
              <span :class="cn('size-1.5 shrink-0 rounded-full', dotClass)" aria-hidden="true" />
              <span class="truncate">
                {{ statusLine(pool, "inspector") }} · {{ updateLabel(update, legacy).suffix }}
              </span>
            </template>
            <template v-else>Not installed</template>
          </p>
        </div>
        <template v-if="isInstalled">
          <Button
            variant="outline"
            :class="INSPECTOR_TOOL"
            :disabled="!canRestart || busy === `restart:${version}`"
            :title="canRestart ? undefined : 'The pool is idle; it starts on the next request'"
            @click="emit('restart', version)"
          >
            <Spinner v-if="busy === `restart:${version}`" class="size-4" />
            <RotateCw v-else />
            Restart pool
          </Button>
          <DropdownMenu v-model:open="headerMenuOpen">
            <DropdownMenuTrigger as-child>
              <Button
                variant="outline"
                size="icon"
                class="size-[34px] shrink-0"
                :aria-label="`More actions for PHP ${version}`"
              >
                <Ellipsis />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" class="w-[248px]">
              <DropdownMenuItem
                :disabled="version === defaultVersion || legacy"
                :title="makeDefaultTitle"
                data-action="make-default"
                @select="emit('makeDefault', version)"
              >
                <Star class="size-4" /> Make default
              </DropdownMenuItem>
              <DropdownMenuItem v-if="update" data-action="update" @select="emit('update', version)">
                <Download class="size-4" /> Update to {{ update.latest }}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem
                class="text-destructive focus:bg-destructive/10 focus:text-destructive"
                data-action="uninstall"
                @select="emit('uninstall', version)"
              >
                <Trash2 class="size-4" /> Uninstall PHP {{ version }}…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </template>
        <Button
          v-if="drawer"
          variant="outline"
          size="icon"
          class="size-[34px] shrink-0"
          aria-label="Close PHP details"
          @click="emit('close')"
        >
          <X />
        </Button>
      </div>

      <PhpVersionPanel
        v-for="v in versions"
        v-show="selected === v"
        :key="v"
        :active="selected === v"
        v-model:layer-open="menuOpenByVersion[v]"
        class="mt-6"
        :data-version-panel="v"
        :version="v"
        :global-settings="data?.settings ?? {}"
        :overrides="data?.version_settings?.[v] ?? {}"
        :directives="data?.directives?.[v] ?? {}"
        :pool="data?.pool?.[v] ?? {}"
        :ffi-available="(data?.ffi_versions ?? []).includes(v)"
        :libvips-found="data?.libvips_found ?? null"
        :xdebug="data?.xdebug?.[v] ?? null"
        :xdebug-available="(data?.xdebug_versions ?? []).includes(v)"
        :extensions="extensionsFor(v)"
        :installed-version="installed.includes(v)"
        :extensions-loading="extensionsLoading"
        @updated="(r) => emit('updated', r)"
        @extensions-updated="(m) => emit('extensionsUpdated', m)"
        @request-add-extension="emit('requestAddExtension', v)"
        @dirty="(d) => emit('dirty', v, d)"
      />
    </div>
  </InspectorFrame>
</template>
