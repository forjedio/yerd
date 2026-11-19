<script setup lang="ts">
import {
  ArrowDown,
  ArrowUp,
  ChevronDown,
  ChevronRight,
  FolderMinus,
  FolderOpen,
  Lock,
  Pencil,
  Star,
} from "lucide-vue-next";
import { computed, nextTick, ref } from "vue";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { SiteEntry } from "@/ipc/types";
import { formatChord } from "@/lib/shortcuts/format";
import { isMac } from "@/lib/shortcuts/platform";
import {
  aliasCount,
  frameworkLabel,
  frameworkRecognised,
  type SiteSection,
  tildify,
  visibleRows,
} from "@/lib/sites";
import { useSitesGroupState } from "@/lib/sitesGroupState";
import { siteHost } from "@/lib/siteUrl";
import { cn } from "@/lib/utils";

/**
 * The grouped site list. `list` is the narrow column beside the inspector (wide
 * windows); `table` spans the window with PHP, Node, framework and TLS columns
 * (compact windows), where clicking a row opens the inspector drawer. Both share
 * the sections, collapse state, "+ N more" overflow and the keyboard: ↑/↓ move
 * the selection, Return opens the site, ⌘E/⌘T open the editor/terminal and ⌘C
 * copies the URL (only when no text is selected).
 */
const props = defineProps<{
  sections: SiteSection[];
  layout: "list" | "table";
  selected: string | null;
  tld: string;
  defaultPhp: string;
  home: string | null;
  /** Each site's effective Node version, by site name (table only). */
  nodeVersions: Record<string, string>;
  filtering: boolean;
  busy: (name: string) => boolean;
  /** The busy token of an in-flight group operation (`group:<name>`), if any. */
  groupBusy: string | null;
  groupOrder: string[];
}>();

const emit = defineEmits<{
  select: [name: string];
  activate: [name: string];
  open: [name: string];
  editor: [name: string];
  terminal: [name: string];
  copyUrl: [name: string];
  moveGroup: [name: string, dir: -1 | 1];
  editGroup: [name: string];
  revealDir: [dir: string];
  unparkDir: [dir: string];
  toggleFavourite: [name: string];
}>();

const { isCollapsed, toggle } = useSitesGroupState();
const expandedOverflow = ref<Set<string>>(new Set());
const root = ref<HTMLElement | null>(null);

function expanded(sec: SiteSection): boolean {
  return props.filtering || !isCollapsed(sec.key);
}

function toggleSection(sec: SiteSection): void {
  if (!props.filtering) toggle(sec.key);
}

function rowsOf(sec: SiteSection) {
  return visibleRows(sec, expandedOverflow.value.has(sec.key), props.filtering);
}

function showAll(sec: SiteSection): void {
  expandedOverflow.value = new Set([...expandedOverflow.value, sec.key]);
}

/** Every row the user can currently see, in order: the keyboard walks these. */
const flatRows = computed<SiteEntry[]>(() =>
  props.sections.flatMap((sec) => (expanded(sec) ? rowsOf(sec).rows : [])),
);

function rowId(name: string): string {
  return `site-row-${name}`;
}

function sectionLabel(sec: SiteSection): string {
  return sec.kind === "dir" && sec.dir ? tildify(sec.dir, props.home) : sec.label;
}

function pathLine(site: SiteEntry): string {
  const path = tildify(site.document_root, props.home);
  return frameworkRecognised(site) ? `${path} · ${frameworkLabel(site)}` : path;
}

function aliasLabel(site: SiteEntry): string {
  const n = aliasCount(site);
  return n === 1 ? "+1 alias" : `+${n} aliases`;
}

function select(name: string): void {
  emit("select", name);
  if (props.layout === "table") emit("activate", name);
}

function moveSelection(delta: 1 | -1): void {
  const rows = flatRows.value;
  if (!rows.length) return;
  const i = rows.findIndex((r) => r.name === props.selected);
  const next = i === -1 ? (delta === 1 ? 0 : rows.length - 1) : i + delta;
  const target = rows[Math.min(Math.max(next, 0), rows.length - 1)];
  if (!target) return;
  emit("select", target.name);
  void nextTick(() => {
    const el = root.value?.querySelector(`[data-site="${target.name}"]`);
    if (el && "scrollIntoView" in el) (el as HTMLElement).scrollIntoView({ block: "nearest" });
  });
}

function hasTextSelection(): boolean {
  return (globalThis.getSelection?.()?.toString() ?? "") !== "";
}

function onKeydown(event: KeyboardEvent): void {
  const mod = event.metaKey || event.ctrlKey;
  if (!mod && event.key === "ArrowDown") {
    event.preventDefault();
    moveSelection(1);
    return;
  }
  if (!mod && event.key === "ArrowUp") {
    event.preventDefault();
    moveSelection(-1);
    return;
  }
  const name = props.selected;
  if (!name) return;
  if (!mod && event.key === "Enter") {
    event.preventDefault();
    emit("open", name);
    return;
  }
  if (!mod || event.shiftKey || event.altKey) return;
  const key = event.key.toLowerCase();
  if (key === "e") {
    event.preventDefault();
    emit("editor", name);
  } else if (key === "t") {
    event.preventDefault();
    emit("terminal", name);
  } else if (key === "c" && !hasTextSelection()) {
    event.preventDefault();
    emit("copyUrl", name);
  }
}

const mac = isMac();
const hints = [
  { keys: "↑↓", label: "navigate" },
  { keys: "Return", label: "open in browser" },
  { keys: formatChord({ mod: true, key: "e" }, mac), label: "editor" },
  { keys: formatChord({ mod: true, key: "t" }, mac), label: "terminal" },
];

const TABLE_COLS = "grid grid-cols-[minmax(0,1fr)_64px_64px_120px_88px_20px] items-center gap-x-3";

function groupIndex(name: string): number {
  return props.groupOrder.indexOf(name);
}
</script>

<template>
  <div class="flex h-full min-h-0 flex-col">
    <div
      ref="root"
      class="group/list min-h-0 flex-1 overflow-y-auto pb-3 pr-4 focus:outline-none"
      tabindex="0"
      role="listbox"
      aria-label="Sites"
      :aria-activedescendant="selected ? rowId(selected) : undefined"
      data-testid="sites-list"
      @keydown="onKeydown"
    >
      <div
        v-if="layout === 'table'"
        :class="cn(TABLE_COLS, 'px-2 pb-1 pt-3 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
        aria-hidden="true"
      >
        <span>Site</span><span>PHP</span><span>Node</span><span>Framework</span><span>TLS</span><span />
      </div>

      <section
        v-for="sec in sections"
        :key="sec.key"
        role="group"
        :aria-label="sectionLabel(sec)"
        :data-section="sec.key"
        class="mt-2"
      >
        <div class="group flex h-8 items-center gap-1.5 px-1">
          <button
            type="button"
            tabindex="-1"
            class="flex min-w-0 flex-1 items-center gap-1.5 text-left"
            :aria-expanded="expanded(sec)"
            @click="toggleSection(sec)"
          >
            <component
              :is="expanded(sec) ? ChevronDown : ChevronRight"
              class="size-3.5 shrink-0 text-faint"
              aria-hidden="true"
            />
            <span
              :class="
                cn(
                  'truncate text-[13px] text-muted-foreground',
                  layout === 'list' && sec.kind !== 'group' ? 'font-mono' : 'font-medium',
                )
              "
            >{{ sectionLabel(sec) }}</span>
            <span class="font-mono text-[11px] text-faint">{{ sec.sites.length }}</span>
          </button>

          <template v-if="sec.kind === 'group' && sec.groupName && !filtering">
            <Spinner v-if="groupBusy === `group:${sec.groupName}`" class="size-3.5" />
            <div
              v-else
              class="pointer-events-none flex items-center gap-0.5 opacity-0 transition-opacity group-hover:pointer-events-auto group-hover:opacity-70 focus-within:pointer-events-auto focus-within:opacity-70"
            >
              <Button
                variant="ghost"
                size="icon-sm"
                :disabled="groupIndex(sec.groupName) === 0"
                :aria-label="`Move ${sec.groupName} up`"
                title="Move up"
                @click="emit('moveGroup', sec.groupName, -1)"
              >
                <ArrowUp class="size-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                :disabled="groupIndex(sec.groupName) === groupOrder.length - 1"
                :aria-label="`Move ${sec.groupName} down`"
                title="Move down"
                @click="emit('moveGroup', sec.groupName, 1)"
              >
                <ArrowDown class="size-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon-sm"
                :aria-label="`Edit ${sec.groupName}`"
                title="Edit group"
                @click="emit('editGroup', sec.groupName)"
              >
                <Pencil class="size-3.5" />
              </Button>
            </div>
          </template>
          <div
            v-else-if="sec.kind === 'dir' && sec.dir"
            class="pointer-events-none flex items-center gap-0.5 opacity-0 transition-opacity group-hover:pointer-events-auto group-hover:opacity-70 focus-within:pointer-events-auto focus-within:opacity-70"
          >
            <Button
              variant="ghost"
              size="icon-sm"
              :aria-label="`Reveal ${sec.dir}`"
              title="Reveal folder"
              @click="emit('revealDir', sec.dir)"
            >
              <FolderOpen class="size-3.5" />
            </Button>
            <Button
              variant="ghost"
              size="icon-sm"
              :aria-label="`Unpark ${sec.dir}`"
              title="Unpark folder…"
              @click="emit('unparkDir', sec.dir)"
            >
              <FolderMinus class="size-3.5" />
            </Button>
          </div>
        </div>

        <template v-if="expanded(sec)">
          <p
            v-if="!sec.sites.length"
            class="px-7 py-2 text-xs text-faint"
          >
            {{ sec.kind === "group" ? "No sites in this group yet." : "No projects in this folder yet." }}
          </p>

          <template v-for="s in rowsOf(sec).rows" :key="s.name">
            <div
              v-if="layout === 'list'"
              :id="rowId(s.name)"
              role="option"
              :aria-selected="s.name === selected"
              :data-site="s.name"
              :class="
                cn(
                  'flex h-10 cursor-pointer items-center gap-2.5 rounded-lg border pl-1.5 pr-3 transition-colors',
                  s.name === selected
                    ? 'border-brand/45 bg-brand/[0.14]'
                    : 'border-transparent hover:bg-accent/60',
                )
              "
              @click="select(s.name)"
            >
              <button
                type="button"
                tabindex="-1"
                :class="
                  cn(
                    'shrink-0 rounded p-0.5 transition-colors',
                    s.favourite ? 'text-warning' : 'text-faint/60 hover:text-foreground',
                  )
                "
                :aria-pressed="s.favourite === true"
                :aria-label="s.favourite ? `Unstar ${s.name}` : `Star ${s.name}`"
                :title="s.favourite ? 'Remove from favourites' : 'Add to favourites'"
                data-testid="row-star"
                @click.stop="emit('toggleFavourite', s.name)"
              >
                <Star :class="cn('size-3.5', s.favourite && 'fill-current')" />
              </button>
              <div class="min-w-0 flex-1">
                <p class="truncate font-mono text-[13px] font-semibold leading-tight">
                  {{ siteHost(s, tld) }}
                </p>
                <p class="truncate text-[11px] leading-tight text-faint">
                  <span v-if="s.apex_shadowed_by" class="text-warning">
                    served by {{ s.apex_shadowed_by }} ·
                  </span>
                  {{ pathLine(s) }}
                </p>
              </div>
              <span v-if="aliasCount(s) > 0" class="shrink-0 font-mono text-[11px] text-faint">
                {{ aliasLabel(s) }}
              </span>
              <Lock v-if="s.secure" class="size-3.5 shrink-0 text-success" aria-label="HTTPS" />
              <Spinner v-if="busy(s.name)" class="size-3.5" />
              <span
                :class="
                  cn(
                    'flex h-5 w-9 shrink-0 items-center justify-center rounded bg-muted font-mono text-[11px] font-medium',
                    defaultPhp && s.php !== defaultPhp ? 'text-warning' : 'text-muted-foreground',
                  )
                "
                data-testid="row-php"
              >
                {{ s.php }}
              </span>
            </div>

            <div
              v-else
              :id="rowId(s.name)"
              role="option"
              :aria-selected="s.name === selected"
              :data-site="s.name"
              :class="
                cn(
                  TABLE_COLS,
                  'h-[42px] cursor-pointer rounded-lg px-2 transition-colors',
                  s.name === selected ? 'hover:bg-accent/60 group-focus-visible/list:bg-brand/25' : 'hover:bg-accent/60',
                )
              "
              @click="select(s.name)"
            >
              <div class="flex min-w-0 items-center gap-2.5">
                <button
                  type="button"
                  tabindex="-1"
                  :class="
                    cn(
                      'shrink-0 rounded p-0.5 transition-colors',
                      s.favourite ? 'text-warning' : 'text-faint/60 hover:text-foreground',
                    )
                  "
                  :aria-pressed="s.favourite === true"
                  :aria-label="s.favourite ? `Unstar ${s.name}` : `Star ${s.name}`"
                  :title="s.favourite ? 'Remove from favourites' : 'Add to favourites'"
                  data-testid="row-star"
                  @click.stop="emit('toggleFavourite', s.name)"
                >
                  <Star :class="cn('size-3.5', s.favourite && 'fill-current')" />
                </button>
                <div class="min-w-0">
                  <p class="truncate font-mono text-[13px] font-semibold leading-tight">
                    {{ siteHost(s, tld) }}
                  </p>
                  <p class="truncate text-[11px] leading-tight text-faint">
                    <span v-if="s.apex_shadowed_by" class="text-warning">
                      served by {{ s.apex_shadowed_by }} ·
                    </span>
                    {{ tildify(s.document_root, home) }}<template v-if="aliasCount(s) > 0"> · {{ aliasLabel(s) }}</template>
                  </p>
                </div>
              </div>
              <span class="flex items-center gap-1.5">
                <span
                  :class="
                    cn(
                      'flex h-5 w-9 items-center justify-center rounded bg-muted font-mono text-[11px] font-medium',
                      defaultPhp && s.php !== defaultPhp ? 'text-warning' : 'text-muted-foreground',
                    )
                  "
                  data-testid="row-php"
                >
                  {{ s.php }}
                </span>
                <Spinner v-if="busy(s.name)" class="size-3.5" />
              </span>
              <span class="font-mono text-xs text-muted-foreground" data-testid="row-node">
                {{ nodeVersions[s.name] ?? "—" }}
              </span>
              <span class="truncate text-xs text-muted-foreground">{{ frameworkLabel(s) }}</span>
              <span class="flex items-center gap-1.5 text-xs">
                <span
                  :class="cn('size-1.5 rounded-full', s.secure ? 'bg-success' : 'bg-faint')"
                  aria-hidden="true"
                />
                <span :class="s.secure ? 'text-foreground' : 'text-faint'">
                  {{ s.secure ? "HTTPS" : "HTTP" }}
                </span>
              </span>
              <button
                type="button"
                tabindex="-1"
                class="text-faint hover:text-foreground"
                :aria-label="`Open details for ${s.name}`"
                @click.stop="select(s.name)"
              >
                <ChevronRight class="size-4" />
              </button>
            </div>
          </template>

          <button
            v-if="rowsOf(sec).hidden > 0"
            type="button"
            tabindex="-1"
            class="px-2 py-1.5 text-xs text-faint hover:text-foreground"
            @click="showAll(sec)"
          >
            + {{ rowsOf(sec).hidden }} more in {{ sectionLabel(sec) }}
          </button>
        </template>
      </section>
    </div>

    <div
      class="mr-4 flex h-9 shrink-0 items-center gap-5 border-t px-2 text-[11px] text-faint"
      aria-hidden="true"
      data-testid="sites-hints"
    >
      <span v-for="h in hints" :key="h.label" class="flex items-center gap-1.5">
        <span class="font-mono text-muted-foreground">{{ h.keys }}</span> {{ h.label }}
      </span>
    </div>
  </div>
</template>
