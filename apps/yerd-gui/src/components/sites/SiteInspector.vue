<script setup lang="ts">
import {
  Code2,
  Copy,
  ExternalLink,
  FolderMinus,
  FolderOpen,
  Lock,
  LockOpen,
  SquareTerminal,
  Star,
  Trash2,
  X,
} from "lucide-vue-next";
import { computed, onMounted, ref } from "vue";

import SiteActionsMenu from "@/components/sites/SiteActionsMenu.vue";
import SiteDomainsCard from "@/components/sites/SiteDomainsCard.vue";
import SiteHeadersCard from "@/components/sites/SiteHeadersCard.vue";
import SiteEnvironmentCard from "@/components/sites/SiteEnvironmentCard.vue";
import Button from "@/components/ui/Button.vue";
import InspectorFooter from "@/components/ui/InspectorFooter.vue";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import { DANGER_TOOL, INSPECTOR_TOOL } from "@/components/ui/styles";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { useSiteEditor } from "@/composables/useSiteEditor";
import { openInBrowser, openInTerminal, openPath } from "@/ipc/client";
import type { GroupsState, SiteEntry, StatusReport } from "@/ipc/types";
import { servedPath, servingState, tildify, urlParts } from "@/lib/sites";
import { siteHost, siteUrl } from "@/lib/siteUrl";
import { copyUrlWithToast, toastOnError } from "@/lib/toastActions";
import { cn } from "@/lib/utils";

/**
 * The selected site's inspector: title, status, the action row, the URL, the
 * Environment and Domains cards and the unpark/remove link. In `panel` mode it is
 * a plain column beside the list (wide windows); in `drawer` mode it slides over
 * the window from the right with a backdrop, closing on Escape, the backdrop or
 * its X. Every write goes up as an event; the Sites view owns the mutations.
 */
const props = defineProps<{
  site: SiteEntry | null;
  mode: "panel" | "drawer";
  /** Drawer mode only: whether the drawer is shown. */
  open?: boolean;
  report: StatusReport | null;
  tld: string;
  home: string | null;
  phpVersions: string[];
  defaultPhp: string;
  groups: GroupsState;
  /** The site's current group, or "" when ungrouped. */
  currentGroup: string;
  busy?: boolean;
  /** Whether a "share publicly" action for this site is in flight. */
  sharing?: boolean;
  /** The parked root serving this site, or null for a linked/unmatched one. */
  parentDir: string | null;
}>();

const emit = defineEmits<{
  close: [];
  changePhp: [site: SiteEntry, version: string];
  changeWebRoot: [site: SiteEntry, path: string];
  toggleSecure: [site: SiteEntry];
  toggleFrontController: [site: SiteEntry, enabled: boolean];
  changeGroup: [site: SiteEntry, group: string];
  changeWpAutoLogin: [site: SiteEntry, enabled: boolean, user: string | null];
  share: [site: SiteEntry];
  unlink: [site: SiteEntry];
  applyProjectFile: [site: SiteEntry];
  exportProjectFile: [site: SiteEntry];
  domainsChanged: [];
  toggleFavourite: [site: SiteEntry];
  createGroupAndMove: [site: SiteEntry, name: string];
  unpark: [dir: string];
  nodeChanged: [];
}>();

const { supportsPathInstall, isMac } = usePlatform();

const drawer = computed(() => props.mode === "drawer");
const shown = computed(() => (drawer.value ? !!props.open : true) && props.site !== null);
const shownSite = computed(() => (shown.value ? props.site : null));

const editor = useSiteEditor(() => shownSite.value);

const layerOpen = ref(false);

function isLayerOpen(): boolean {
  return layerOpen.value;
}

const url = computed(() => (props.site ? siteUrl(props.site, props.report) : ""));
const urlSplit = computed(() => urlParts(url.value));
const status = computed(() => (props.site ? servingState(props.site, props.report) : null));

function openSite(site: SiteEntry): void {
  void toastOnError("Couldn't open site", () => openInBrowser(siteUrl(site, props.report)));
}

function openTerminal(site: SiteEntry): void {
  void toastOnError("Couldn't open terminal", () => openInTerminal(site.document_root));
}

function reveal(site: SiteEntry): void {
  void toastOnError("Couldn't reveal site folder", () => openPath(site.document_root));
}

async function copyUrl(): Promise<void> {
  await copyUrlWithToast(url.value);
}

function changeIde(value: string): void {
  if (props.site) void editor.changeIde(props.site, value);
}

onMounted(() => {
  void loadPlatform();
});

</script>

<template>
  <InspectorFrame
    :mode="mode"
    :shown="shown && site !== null"
    label="Site details"
    testid="site-inspector"
    backdrop-class="site-inspector-backdrop"
    :layer-open="isLayerOpen"
    @close="emit('close')"
  >
    <template v-if="site">
      <div class="flex flex-1 flex-col px-7 pb-6 pt-7">
        <div class="flex items-start gap-3">
          <div class="min-w-0 flex-1">
            <h2 class="truncate font-mono text-[22px] font-semibold leading-tight tracking-[-0.2px]">
              {{ siteHost(site, tld) }}
            </h2>
            <button
              type="button"
              class="mt-1.5 flex max-w-full items-center gap-1.5 text-faint hover:text-foreground"
              :title="`Reveal ${site.document_root}`"
              data-testid="inspector-path"
              @click="reveal(site)"
            >
              <FolderOpen class="size-3 shrink-0" aria-hidden="true" />
              <span class="truncate font-mono text-xs text-muted-foreground">
                {{ tildify(servedPath(site), home) }}
              </span>
            </button>
          </div>
          <span
            v-if="status"
            data-testid="serving-pill"
            :class="
              cn(
                'mt-1.5 inline-flex shrink-0 items-center gap-1.5 text-xs font-semibold',
                status.tone === 'ok' && 'text-success',
                status.tone === 'warn' && 'text-warning',
                status.tone === 'bad' && 'text-destructive',
              )
            "
          >
            <span class="size-1.5 rounded-full bg-current" aria-hidden="true" />
            {{ status.label }}
          </span>
          <Button
            v-if="drawer"
            variant="outline"
            size="icon"
            class="size-8 shrink-0"
            aria-label="Close site details"
            @click="emit('close')"
          >
            <X />
          </Button>
        </div>

        <div class="mt-5 flex flex-wrap items-center gap-2">
          <Button :class="INSPECTOR_TOOL" @click="openSite(site)">
            <ExternalLink /> Open site
          </Button>
          <Button
            variant="outline"
            size="icon"
            :class="
              cn(
                'size-[34px]',
                site.favourite && 'border-warning/60 bg-warning/15 text-warning hover:bg-warning/20 hover:text-warning',
              )
            "
            :aria-pressed="site.favourite === true"
            :aria-label="site.favourite ? `Unstar ${site.name}` : `Star ${site.name}`"
            :title="site.favourite ? 'Remove from favourites' : 'Add to favourites'"
            @click="emit('toggleFavourite', site)"
          >
            <Star :class="site.favourite ? 'fill-current' : ''" />
          </Button>
          <Button
            v-if="supportsPathInstall"
            variant="outline"
            :class="INSPECTOR_TOOL"
            :title="editor.editorTooltip.value"
            @click="editor.openEditor(site)"
          >
            <Code2 /> Editor
          </Button>
          <Button variant="outline" :class="INSPECTOR_TOOL" @click="openTerminal(site)">
            <SquareTerminal /> Terminal
          </Button>
          <Button variant="outline" :class="INSPECTOR_TOOL" @click="reveal(site)">
            <FolderOpen /> {{ isMac ? "Finder" : "Files" }}
          </Button>
          <SiteActionsMenu
            v-model:layer-open="layerOpen"
            :site="site"
            :report="report"
            :tld="tld"
            :home="home"
            :groups="groups"
            :current-group="currentGroup"
            :parent-dir="parentDir"
            :sharing="sharing"
            :busy="busy"
            :editor-available="supportsPathInstall"
            :ide-options="editor.ideOptions.value"
            :selected-ide="editor.selectedIde.value"
            @share="(s) => emit('share', s)"
            @change-web-root="(s, path) => emit('changeWebRoot', s, path)"
            @toggle-front-controller="(s, on) => emit('toggleFrontController', s, on)"
            @change-group="(s, g) => emit('changeGroup', s, g)"
            @create-group-and-move="(s, name) => emit('createGroupAndMove', s, name)"
            @unpark="(dir) => emit('unpark', dir)"
            @unlink="(s) => emit('unlink', s)"
            @apply-project-file="(s) => emit('applyProjectFile', s)"
            @export-project-file="(s) => emit('exportProjectFile', s)"
            @change-ide="changeIde"
          />
        </div>

        <div
          class="mt-4 flex h-10 items-center gap-2.5 rounded-lg border bg-background px-3.5"
          data-testid="inspector-url"
        >
          <Lock v-if="site.secure" class="size-3.5 shrink-0 text-success" aria-label="Secured" />
          <LockOpen v-else class="size-3.5 shrink-0 text-faint" aria-label="Not secured" />
          <span class="min-w-0 flex-1 truncate font-mono text-[13px]">
            {{ urlSplit.text }}<span v-if="urlSplit.port" class="text-warning">:{{ urlSplit.port }}</span>{{ urlSplit.path }}
          </span>
          <button
            type="button"
            class="shrink-0 rounded p-1 text-faint transition-colors hover:text-foreground"
            aria-label="Copy URL"
            title="Copy URL"
            @click="copyUrl"
          >
            <Copy class="size-3.5" />
          </button>
        </div>

        <div class="mt-7 space-y-7">
          <SiteEnvironmentCard
            :site="site"
            :report="report"
            :php-versions="phpVersions"
            :default-php="defaultPhp"
            :home="home"
            :parent-dir="parentDir"
            :busy="busy"
            @change-php="(s, v) => emit('changePhp', s, v)"
            @toggle-secure="(s) => emit('toggleSecure', s)"
            @change-wp-auto-login="(s, on, user) => emit('changeWpAutoLogin', s, on, user)"
            @node-changed="emit('nodeChanged')"
          />
          <SiteDomainsCard :site="site" :tld="tld" @domains-changed="emit('domainsChanged')" />
          <SiteHeadersCard :site="site" />
        </div>

        <InspectorFooter v-if="(site.kind === 'parked' && parentDir) || site.kind === 'linked'">
          <Button
            v-if="site.kind === 'parked' && parentDir"
            variant="outline"
            :class="DANGER_TOOL"
            data-testid="inspector-footer-action"
            :title="`Stops serving every site in ${tildify(parentDir, home)}`"
            @click="emit('unpark', parentDir)"
          >
            <FolderMinus /> Unpark folder…
          </Button>
          <Button
            v-else
            variant="outline"
            :class="DANGER_TOOL"
            :disabled="busy"
            data-testid="inspector-footer-action"
            @click="emit('unlink', site)"
          >
            <Trash2 /> Remove site…
          </Button>
        </InspectorFooter>
      </div>
    </template>
  </InspectorFrame>
</template>

