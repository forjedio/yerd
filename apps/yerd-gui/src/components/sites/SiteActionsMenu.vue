<script setup lang="ts">
import {
  Check,
  Code2,
  Copy,
  Ellipsis,
  FileCog,
  FileDown,
  FolderMinus,
  FolderTree,
  Gauge,
  Layers,
  Plus,
  Route,
  Share2,
  Trash2,
  UserRound,
} from "lucide-vue-next";
import { PopoverAnchor, PopoverContent, PopoverPortal, PopoverRoot } from "reka-ui";
import { computed, nextTick, onBeforeUnmount, ref, watch } from "vue";
import { useRouter } from "vue-router";

import SiteRoutesPanel from "@/components/SiteRoutesPanel.vue";
import WebRootDialog from "@/components/sites/WebRootDialog.vue";
import Button from "@/components/ui/Button.vue";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import Input from "@/components/ui/Input.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import Switch from "@/components/ui/Switch.vue";
import { rememberInsightsSite } from "@/composables/useInsights";
import type { GroupsState, SiteEntry, StatusReport } from "@/ipc/types";
import { formatChord } from "@/lib/shortcuts/format";
import { isMac } from "@/lib/shortcuts/platform";
import { groupNameAvailable, tildify } from "@/lib/sites";
import { siteHost, siteUrl } from "@/lib/siteUrl";
import { copyUrlWithToast } from "@/lib/toastActions";
import { openWpAdmin } from "@/lib/wpAdmin";

/**
 * The inspector's `···` menu. It holds the actions the draft draws (Copy URL,
 * Share publicly, Change web root, Move to group with its New group popover,
 * Routing rules, Unpark or Remove) plus those the old details sidebar had and
 * the draft does not (the Editor override, WP Admin or Insights).
 * Dialogs opened from here open on the next tick so the menu's focus restore
 * doesn't fight them.
 *
 * `layerOpen` is true while the dropdown or the New group popover is open, so
 * the drawer can leave Escape to that layer: reka never prevents Escape and
 * listens on `window`, after the drawer's document listener. It is cleared on
 * unmount too, since a menu torn down while open never reports closing.
 */
const props = defineProps<{
  site: SiteEntry;
  report: StatusReport | null;
  tld: string;
  home: string | null;
  groups: GroupsState;
  /** The site's current group, or "" when ungrouped. */
  currentGroup: string;
  /** The parked root serving this site, or null for a linked/unmatched one. */
  parentDir: string | null;
  sharing?: boolean;
  busy?: boolean;
  /** Whether the host can launch editors (macOS and Linux). */
  editorAvailable: boolean;
  ideOptions: { value: string; label: string }[];
  selectedIde: string;
}>();

const emit = defineEmits<{
  share: [site: SiteEntry];
  changeWebRoot: [site: SiteEntry, path: string];
  toggleFrontController: [site: SiteEntry, enabled: boolean];
  changeGroup: [site: SiteEntry, group: string];
  createGroupAndMove: [site: SiteEntry, name: string];
  unpark: [dir: string];
  unlink: [site: SiteEntry];
  applyProjectFile: [site: SiteEntry];
  exportProjectFile: [site: SiteEntry];
  changeIde: [value: string];
}>();

const layerOpen = defineModel<boolean>("layerOpen", { default: false });

const router = useRouter();

const copyHint = formatChord({ mod: true, key: "c" }, isMac());

const host = computed(() => siteHost(props.site, props.tld));
const menuOpen = ref(false);
const newGroupOpen = ref(false);
const newGroupName = ref("");
const webRootOpen = ref(false);
const routingOpen = ref(false);

watch([menuOpen, newGroupOpen], ([menu, popover]) => {
  layerOpen.value = menu || popover;
});

onBeforeUnmount(() => {
  layerOpen.value = false;
});

/** Set when New group… is picked: the closing menu must then skip its focus
 *  restore, or refocusing the `···` trigger counts as a focus-outside for the
 *  non-modal popover and dismisses it as soon as it opens. */
let keepFocusOnClose = false;

function onMenuCloseAutoFocus(event: Event): void {
  if (!keepFocusOnClose) return;
  keepFocusOnClose = false;
  event.preventDefault();
}

const newGroupValid = computed(() => groupNameAvailable(newGroupName.value, props.groups.order));

function later(open: () => void): void {
  void nextTick(open);
}

function openNewGroup(): void {
  keepFocusOnClose = true;
  newGroupName.value = "";
  later(() => {
    newGroupOpen.value = true;
  });
}

function submitNewGroup(): void {
  if (!newGroupValid.value) return;
  emit("createGroupAndMove", props.site, newGroupName.value.trim());
  newGroupOpen.value = false;
}

function moveTo(group: string): void {
  if (group !== props.currentGroup) emit("changeGroup", props.site, group);
}

async function copyUrl(): Promise<void> {
  await copyUrlWithToast(siteUrl(props.site, props.report));
}

function openInsights(): void {
  rememberInsightsSite(props.site.name);
  void router.push("/insights");
}
</script>

<template>
  <PopoverRoot v-model:open="newGroupOpen">
    <PopoverAnchor class="inline-flex">
      <DropdownMenu v-model:open="menuOpen">
        <DropdownMenuTrigger as-child>
          <Button
            variant="outline"
            size="icon"
            class="size-[34px]"
            :aria-label="`More actions for ${site.name}`"
          >
            <Ellipsis />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" class="w-60" @close-auto-focus="onMenuCloseAutoFocus">
          <DropdownMenuItem @select="copyUrl">
            <Copy class="size-4" /> Copy URL
            <span class="ml-auto text-[11px] text-faint">{{ copyHint }}</span>
          </DropdownMenuItem>
          <DropdownMenuItem :disabled="sharing" @select="emit('share', site)">
            <Spinner v-if="sharing" class="size-4" />
            <Share2 v-else class="size-4" />
            Share publicly…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem v-if="!site.is_wordpress" @select="later(() => (webRootOpen = true))">
            <FolderTree class="size-4" /> Change web root…
          </DropdownMenuItem>
          <DropdownMenuSub>
            <DropdownMenuSubTrigger>
              <Layers class="size-4" /> Move to group
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent class="w-52">
              <DropdownMenuItem data-group="" @select="moveTo('')">
                <Check :class="currentGroup === '' ? 'size-4' : 'size-4 opacity-0'" />
                No group
              </DropdownMenuItem>
              <DropdownMenuItem
                v-for="g in groups.order"
                :key="g"
                :data-group="g"
                @select="moveTo(g)"
              >
                <Check :class="currentGroup === g ? 'size-4' : 'size-4 opacity-0'" />
                <span class="truncate">{{ g }}</span>
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem @select="openNewGroup">
                <Plus class="size-4" /> New group…
              </DropdownMenuItem>
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuItem @select="later(() => (routingOpen = true))">
            <Route class="size-4" /> Routing rules…
          </DropdownMenuItem>
          <DropdownMenuItem @select="emit('applyProjectFile', site)">
            <FileCog class="size-4" /> Apply .yerd.toml
          </DropdownMenuItem>
          <DropdownMenuItem @select="emit('exportProjectFile', site)">
            <FileDown class="size-4" /> Export to .yerd.toml
          </DropdownMenuItem>
          <DropdownMenuSub v-if="editorAvailable">
            <DropdownMenuSubTrigger>
              <Code2 class="size-4" /> Editor
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent class="w-56">
              <DropdownMenuItem
                v-for="o in ideOptions"
                :key="o.value"
                :data-ide="o.value"
                @select="emit('changeIde', o.value)"
              >
                <Check :class="selectedIde === o.value ? 'size-4' : 'size-4 opacity-0'" />
                <span class="truncate">{{ o.label }}</span>
              </DropdownMenuItem>
            </DropdownMenuSubContent>
          </DropdownMenuSub>
          <DropdownMenuItem v-if="site.is_wordpress" @select="openWpAdmin(site, report)">
            <UserRound class="size-4" /> WP Admin
          </DropdownMenuItem>
          <DropdownMenuItem v-else @select="openInsights">
            <Gauge class="size-4" /> Insights
          </DropdownMenuItem>
          <template v-if="(site.kind === 'parked' && parentDir) || site.kind === 'linked'">
            <DropdownMenuSeparator />
            <DropdownMenuItem
              v-if="site.kind === 'parked' && parentDir"
              class="text-destructive focus:text-destructive"
              @select="emit('unpark', parentDir)"
            >
              <FolderMinus class="size-4" /> Unpark {{ tildify(parentDir, home) }}…
            </DropdownMenuItem>
            <DropdownMenuItem
              v-else
              class="text-destructive focus:text-destructive"
              @select="emit('unlink', site)"
            >
              <Trash2 class="size-4" /> Remove site…
            </DropdownMenuItem>
          </template>
        </DropdownMenuContent>
      </DropdownMenu>
    </PopoverAnchor>

    <PopoverPortal>
      <PopoverContent
        align="end"
        :side-offset="6"
        class="z-[60] w-[248px] rounded-[10px] border bg-popover p-3.5 text-popover-foreground shadow-md focus:outline-none"
      >
        <form v-if="newGroupOpen" data-testid="new-group-popover" @submit.prevent="submitNewGroup">
          <p class="text-[13px] font-semibold">New group</p>
          <Input
            v-model="newGroupName"
            class="mt-2.5"
            placeholder="e.g. Client work"
            aria-label="New group name"
            autofocus
          />
          <p class="mt-2 text-[11px] text-faint">Only changes how the list is sorted.</p>
          <div class="mt-3 flex justify-end gap-2">
            <Button variant="ghost" size="sm" @click="newGroupOpen = false">Cancel</Button>
            <Button size="sm" type="submit" :disabled="!newGroupValid">Create &amp; move</Button>
          </div>
        </form>
      </PopoverContent>
    </PopoverPortal>
  </PopoverRoot>

  <WebRootDialog
    v-model:open="webRootOpen"
    :site="site"
    :busy="busy"
    @change-web-root="(s, path) => emit('changeWebRoot', s, path)"
  />

  <Modal v-model:open="routingOpen" :title="`Routing rules · ${host}`" size="lg">
    <template v-if="!site.is_wordpress && site.uses_front_controller !== undefined">
      <div class="rounded-lg border px-3 py-3">
        <div class="flex items-center justify-between gap-4">
          <div>
            <p class="text-sm font-medium">Route through front controller</p>
            <p class="mt-1 text-xs text-muted-foreground">
              On: every request funnels through the site's <code class="font-mono">index.php</code>
              (Laravel, Symfony). Off: named <code class="font-mono">.php</code> files are executed directly
              (plain PHP).
            </p>
          </div>
          <Switch
            :model-value="site.uses_front_controller"
            :disabled="busy"
            aria-label="Route through front controller"
            @update:model-value="emit('toggleFrontController', site, $event)"
          />
        </div>
      </div>
      <hr class="my-4" />
    </template>
    <SiteRoutesPanel :site="site" />
  </Modal>

</template>
