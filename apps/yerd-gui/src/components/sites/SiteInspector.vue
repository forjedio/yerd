<script setup lang="ts">
import {
  Code2,
  Copy,
  ExternalLink,
  FolderOpen,
  Lock,
  LockOpen,
  SquareTerminal,
  Star,
  X,
} from "lucide-vue-next";
import { computed, onMounted, onUnmounted, ref } from "vue";

import SiteActionsMenu from "@/components/sites/SiteActionsMenu.vue";
import SiteDomainsCard from "@/components/sites/SiteDomainsCard.vue";
import SiteEnvironmentCard from "@/components/sites/SiteEnvironmentCard.vue";
import Button from "@/components/ui/Button.vue";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { useSiteEditor } from "@/composables/useSiteEditor";
import { useToast } from "@/composables/useToast";
import { IpcError, openInBrowser, openInTerminal, openPath } from "@/ipc/client";
import type { GroupsState, SiteEntry, StatusReport } from "@/ipc/types";
import { servedPath, servingState, tildify, urlParts } from "@/lib/sites";
import { siteHost, siteUrl } from "@/lib/siteUrl";
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
  changeWpAutoLogin: [
    site: SiteEntry,
    enabled: boolean,
    user: string | null,
    options?: { silent?: boolean },
  ];
  share: [site: SiteEntry];
  unlink: [site: SiteEntry];
  domainsChanged: [];
  toggleFavourite: [site: SiteEntry];
  createGroupAndMove: [site: SiteEntry, name: string];
  unpark: [dir: string];
  nodeChanged: [];
}>();

const toast = useToast();
const { supportsPathInstall, isMac } = usePlatform();

const drawer = computed(() => props.mode === "drawer");
const shown = computed(() => (drawer.value ? !!props.open : true) && props.site !== null);
const shownSite = computed(() => (shown.value ? props.site : null));

const editor = useSiteEditor(() => shownSite.value);

const layerOpen = ref(false);

const url = computed(() => (props.site ? siteUrl(props.site, props.report) : ""));
const urlSplit = computed(() => urlParts(url.value));
const status = computed(() => (props.site ? servingState(props.site, props.report) : null));

async function run(action: () => Promise<void>, failure: string): Promise<void> {
  try {
    await action();
  } catch (error) {
    toast.error(failure, (error as IpcError).message);
  }
}

function openSite(site: SiteEntry): void {
  void run(() => openInBrowser(siteUrl(site, props.report)), "Couldn't open site");
}

function openTerminal(site: SiteEntry): void {
  void run(() => openInTerminal(site.document_root), "Couldn't open terminal");
}

function reveal(site: SiteEntry): void {
  void run(() => openPath(site.document_root), "Couldn't reveal site folder");
}

async function copyUrl(): Promise<void> {
  try {
    await navigator.clipboard.writeText(url.value);
    toast.success("Copied URL");
  } catch {
    toast.error("Couldn't copy URL", "Your browser blocked clipboard access.");
  }
}

/** Forwards the card's arguments as given, so an absent `options` stays absent. */
function forwardWpAutoLogin(
  ...args: [site: SiteEntry, enabled: boolean, user: string | null, options?: { silent?: boolean }]
): void {
  emit("changeWpAutoLogin", ...args);
}

function changeIde(value: string): void {
  if (props.site) void editor.changeIde(props.site, value);
}

const panel = ref<HTMLElement | null>(null);

/** Whether another modal layer sits above the drawer. Dialogs opened from inside
 *  it (Routing rules, web root, Domains) and the Sites view's confirms teleport
 *  into `<body>` after the drawer, so a later dialog in document order sits on
 *  top and owns Escape: without this both listeners fire and one keypress
 *  dismisses the pair. A drawer that isn't in the document (a detached test
 *  mount) has index -1, so any other open dialog counts as being above it. */
function hasDialogAbove(): boolean {
  const dialogs = Array.from(
    document.querySelectorAll('[role="dialog"][aria-modal="true"], [role="alertdialog"]'),
  );
  const own = panel.value ? dialogs.indexOf(panel.value) : -1;
  return dialogs.some((dialog, index) => dialog !== panel.value && index > own);
}

function onKeydown(event: KeyboardEvent): void {
  if (!drawer.value || !shown.value || event.key !== "Escape") return;
  if (layerOpen.value || hasDialogAbove()) return;
  emit("close");
}

onMounted(() => {
  void loadPlatform();
  document.addEventListener("keydown", onKeydown);
});
onUnmounted(() => {
  document.removeEventListener("keydown", onKeydown);
});

const TOOL = "h-[34px] gap-1.5 px-3 text-[13px] font-semibold";
</script>

<template>
  <Teleport to="body" :disabled="!drawer">
    <Transition name="site-inspector-backdrop">
      <div
        v-if="drawer && shown"
        class="site-inspector-backdrop fixed inset-0 z-40 rounded-[10px] bg-black/50 [html.window-maximized_&]:rounded-none"
        aria-hidden="true"
        @click="emit('close')"
      />
    </Transition>

    <!-- In drawer mode a teleported overlay escapes the window's rounded clip, so
         this wrapper restores it over the full window height. It stays mounted,
         paint-free, for the leave transition. -->
    <div
      :class="
        drawer
          ? 'pointer-events-none fixed inset-0 z-50 overflow-hidden rounded-[10px] [html.window-maximized_&]:rounded-none'
          : 'h-full'
      "
    >
      <Transition :name="drawer ? 'site-inspector-panel' : 'site-inspector-none'">
        <aside
          v-if="shown && site"
          ref="panel"
          :class="
            cn(
              'flex flex-col overflow-y-auto bg-background',
              drawer
                ? 'pointer-events-auto absolute inset-y-0 right-0 w-[628px] max-w-full border-l shadow-2xl'
                : 'h-full',
            )
          "
          aria-label="Site details"
          :role="drawer ? 'dialog' : undefined"
          :aria-modal="drawer ? 'true' : undefined"
          data-testid="site-inspector"
          @click.stop
        >
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
              <Button :class="TOOL" @click="openSite(site)">
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
                :class="TOOL"
                :title="editor.editorTooltip.value"
                @click="editor.openEditor(site)"
              >
                <Code2 /> Editor
              </Button>
              <Button variant="outline" :class="TOOL" @click="openTerminal(site)">
                <SquareTerminal /> Terminal
              </Button>
              <Button variant="outline" :class="TOOL" @click="reveal(site)">
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
                @change-wp-auto-login="forwardWpAutoLogin"
                @node-changed="emit('nodeChanged')"
              />
              <SiteDomainsCard :site="site" :tld="tld" @domains-changed="emit('domainsChanged')" />
            </div>

            <div class="mt-auto flex justify-end pt-8">
              <button
                v-if="site.kind === 'parked' && parentDir"
                type="button"
                class="text-xs font-medium text-destructive hover:underline"
                data-testid="inspector-footer-action"
                @click="emit('unpark', parentDir)"
              >
                Unpark site…
              </button>
              <button
                v-else-if="site.kind === 'linked'"
                type="button"
                class="text-xs font-medium text-destructive hover:underline disabled:opacity-50"
                :disabled="busy"
                data-testid="inspector-footer-action"
                @click="emit('unlink', site)"
              >
                Remove site…
              </button>
            </div>
          </div>
        </aside>
      </Transition>
    </div>
  </Teleport>
</template>

<style scoped>
.site-inspector-backdrop-enter-active,
.site-inspector-backdrop-leave-active {
  transition: opacity 180ms ease;
}

.site-inspector-backdrop-enter-from,
.site-inspector-backdrop-leave-to {
  opacity: 0;
}

.site-inspector-panel-enter-active,
.site-inspector-panel-leave-active {
  transition: transform 220ms ease;
}

.site-inspector-panel-enter-from,
.site-inspector-panel-leave-to {
  transform: translateX(100%);
}
</style>
