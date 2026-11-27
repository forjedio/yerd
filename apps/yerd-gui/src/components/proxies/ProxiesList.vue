<script setup lang="ts">
import { ChevronRight, CornerDownRight, Globe, Lock, LockOpen } from "lucide-vue-next";
import { computed, nextTick, ref } from "vue";

import Spinner from "@/components/ui/Spinner.vue";
import type { SiteEntry } from "@/ipc/types";
import {
  extraDomains,
  proxyHost,
  type ProxyItem,
  type ProxySection,
  ruleSite,
  upstreamLabel,
} from "@/lib/proxies";
import { aliasCount } from "@/lib/sites";
import { cn } from "@/lib/utils";

/**
 * The Proxies list. `list` is the column beside the inspector (wide windows):
 * two-line rows with the upstream, an alias count and a lock. `table` spans the
 * window with FORWARDS TO, DOMAINS and TLS columns (compact windows), where
 * clicking a row opens the inspector drawer. ↑/↓ move the selection and Return
 * activates it.
 */
const props = defineProps<{
  sections: ProxySection[];
  layout: "list" | "table";
  selected: string | null;
  tld: string;
  sites: SiteEntry[];
  busy: (key: string) => boolean;
}>();

const emit = defineEmits<{
  select: [key: string];
  activate: [key: string];
}>();

const root = ref<HTMLElement | null>(null);

const flatKeys = computed(() => props.sections.flatMap((s) => s.items.map((i) => i.key)));

function rowId(key: string): string {
  return `proxy-row-${key}`;
}

function select(key: string): void {
  emit("select", key);
  if (props.layout === "table") emit("activate", key);
}

function moveSelection(delta: 1 | -1): void {
  const keys = flatKeys.value;
  if (!keys.length) return;
  const i = props.selected ? keys.indexOf(props.selected) : -1;
  const next = i === -1 ? (delta === 1 ? 0 : keys.length - 1) : i + delta;
  const target = keys[Math.min(Math.max(next, 0), keys.length - 1)];
  if (!target) return;
  emit("select", target);
  void nextTick(() => {
    const rows = Array.from(root.value?.querySelectorAll<HTMLElement>("[data-key]") ?? []);
    rows.find((el) => el.dataset.key === target)?.scrollIntoView?.({ block: "nearest" });
  });
}

function onKeydown(event: KeyboardEvent): void {
  if (event.metaKey || event.ctrlKey || event.altKey) return;
  if (event.key === "ArrowDown" || event.key === "ArrowUp") {
    event.preventDefault();
    moveSelection(event.key === "ArrowDown" ? 1 : -1);
  } else if (event.key === "Enter" && props.selected) {
    event.preventDefault();
    emit("activate", props.selected);
  }
}

function secure(item: ProxyItem): boolean {
  return item.kind === "proxy" ? item.proxy.secure : ruleSite(item.rule, props.sites)?.secure === true;
}

function target(item: ProxyItem): string {
  return upstreamLabel(item.kind === "proxy" ? item.proxy.target : item.rule.target);
}

function ariaLabel(item: ProxyItem): string {
  return item.kind === "proxy"
    ? proxyHost(item.proxy, props.tld)
    : `${item.rule.site}.${props.tld}${item.rule.prefix}`;
}

const TABLE_COLS =
  "grid grid-cols-[minmax(0,1fr)_minmax(0,200px)_minmax(0,150px)_72px_20px] items-center gap-x-3";
</script>

<template>
  <div
    ref="root"
    class="group/list min-h-0 overflow-y-auto pb-3 pr-4 focus:outline-none"
    tabindex="0"
    role="listbox"
    aria-label="Proxies"
    :aria-activedescendant="selected ? rowId(selected) : undefined"
    data-testid="proxies-list"
    @keydown="onKeydown"
  >
    <div
      v-if="layout === 'table'"
      :class="cn(TABLE_COLS, 'border-b px-2 pb-2 pt-3 text-[11px] font-medium uppercase tracking-[0.6px] text-faint')"
      aria-hidden="true"
    >
      <span>Proxy</span><span>Forwards to</span><span>Domains</span><span>TLS</span><span />
    </div>

    <section
      v-for="sec in sections"
      :key="sec.kind"
      role="group"
      :aria-label="sec.label"
      :data-section="sec.kind"
      class="mt-2"
    >
      <div class="flex h-8 items-center gap-1.5 px-1">
        <template v-if="layout === 'list'">
          <Globe v-if="sec.kind === 'proxy'" class="size-[13px] shrink-0 text-faint" aria-hidden="true" />
          <CornerDownRight v-else class="size-[13px] shrink-0 text-faint" aria-hidden="true" />
        </template>
        <span class="text-[13px] font-medium text-muted-foreground">
          {{ layout === "table" && sec.kind === "proxy" ? "Whole domains" : sec.label }}
        </span>
        <span class="font-mono text-[11px] text-faint">{{ sec.items.length }}</span>
      </div>

      <template v-for="item in sec.items" :key="item.key">
        <div
          v-if="layout === 'list'"
          :id="rowId(item.key)"
          role="option"
          :aria-selected="item.key === selected"
          :aria-label="ariaLabel(item)"
          :data-key="item.key"
          :class="
            cn(
              'flex h-[50px] cursor-pointer items-center gap-3 rounded-lg border px-2 transition-colors',
              item.key === selected
                ? 'border-brand/45 bg-brand/[0.14]'
                : 'border-transparent hover:bg-accent/60',
            )
          "
          @click="select(item.key)"
        >
          <div class="min-w-0 flex-1">
            <p class="truncate font-mono text-[13px] font-semibold leading-tight">
              <template v-if="item.kind === 'proxy'">{{ proxyHost(item.proxy, tld) }}</template>
              <template v-else>
                <span class="font-normal text-muted-foreground">{{ item.rule.site }}.{{ tld }}</span>{{ item.rule.prefix }}
              </template>
            </p>
            <p class="mt-1 truncate font-mono text-xs leading-tight text-faint">→ {{ target(item) }}</p>
          </div>
          <span
            v-if="item.kind === 'proxy' && aliasCount(item.proxy) > 0"
            class="shrink-0 font-mono text-[11px] text-faint"
            data-testid="row-aliases"
          >
            +{{ aliasCount(item.proxy) }}
          </span>
          <Spinner v-if="busy(item.key)" class="size-3.5 shrink-0" />
          <Lock v-else-if="secure(item)" class="size-[13px] shrink-0 text-success" aria-label="HTTPS" />
          <LockOpen v-else class="size-[13px] shrink-0 text-faint" aria-label="HTTP" />
        </div>

        <div
          v-else
          :id="rowId(item.key)"
          role="option"
          :aria-selected="item.key === selected"
          :aria-label="ariaLabel(item)"
          :data-key="item.key"
          :class="
            cn(
              TABLE_COLS,
              'h-[46px] cursor-pointer rounded-lg px-2 transition-colors hover:bg-accent/60',
              item.key === selected && 'group-focus-visible/list:bg-brand/25',
            )
          "
          @click="select(item.key)"
        >
          <div class="min-w-0">
            <p class="truncate font-mono text-[13px] font-semibold leading-tight">
              <template v-if="item.kind === 'proxy'">{{ proxyHost(item.proxy, tld) }}</template>
              <template v-else>
                <span class="font-normal text-muted-foreground">{{ item.rule.site }}.{{ tld }}</span>{{ item.rule.prefix }}
              </template>
            </p>
            <p class="mt-0.5 truncate text-[11px] leading-tight text-faint">
              {{ item.kind === "proxy" ? "Whole domain" : "Path on a site" }}
            </p>
          </div>
          <span class="flex min-w-0 items-center gap-1.5">
            <span class="truncate font-mono text-xs text-muted-foreground">{{ target(item) }}</span>
            <Spinner v-if="busy(item.key)" class="size-3.5 shrink-0" />
          </span>
          <span class="flex min-w-0 items-center gap-1.5 text-xs" data-testid="row-domains">
            <template v-if="item.kind === 'rule'">
              <span class="truncate text-faint">site domains</span>
            </template>
            <template v-else-if="extraDomains(item.proxy, tld).length">
              <span class="truncate font-mono text-muted-foreground">{{ extraDomains(item.proxy, tld)[0] }}</span>
              <span
                v-if="extraDomains(item.proxy, tld).length > 1"
                class="shrink-0 rounded bg-muted px-1.5 font-mono text-[11px] text-muted-foreground"
              >
                +{{ extraDomains(item.proxy, tld).length - 1 }}
              </span>
            </template>
            <span v-else class="text-faint">—</span>
          </span>
          <span class="flex items-center gap-1.5 text-xs">
            <span
              :class="cn('size-1.5 rounded-full', secure(item) ? 'bg-success' : 'bg-faint')"
              aria-hidden="true"
            />
            <span :class="secure(item) ? 'text-foreground' : 'text-faint'">
              {{ secure(item) ? "HTTPS" : "HTTP" }}
            </span>
          </span>
          <button
            type="button"
            tabindex="-1"
            class="text-faint hover:text-foreground"
            :aria-label="`Open details for ${ariaLabel(item)}`"
            @click.stop="select(item.key)"
          >
            <ChevronRight class="size-4" />
          </button>
        </div>
      </template>
    </section>
  </div>
</template>
