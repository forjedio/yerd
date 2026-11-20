<script setup lang="ts">
import { computed } from "vue";
import { Copy, ExternalLink, Info, KeyRound, RotateCw, Square, TriangleAlert, X } from "lucide-vue-next";

import { CARD } from "@/components/ui/styles";
import Button from "@/components/ui/Button.vue";
import InspectorFrame from "@/components/ui/InspectorFrame.vue";
import Spinner from "@/components/ui/Spinner.vue";
import type { CloudflaredStatus, Site } from "@/ipc/types";
import { scheme, type ShareItem, servedOver, stateDotClass, stateLabel, statusDetail, typeFacts } from "@/lib/share";
import { cn } from "@/lib/utils";

/**
 * The Share inspector: the selected share's header (kind chip, Stop sharing for
 * a quick link, Restart tunnel and Remove from domain for a domain share, the
 * share password), the public URL card with Copy and Open, the TYPE / LOCAL
 * SITE / STATUS strip, any preflight warnings and, for a quick link, its
 * limits. In `panel` mode it sits beside the list; in `drawer` mode it slides
 * over the window (760px).
 */
const props = defineProps<{
  mode: "panel" | "drawer";
  shown: boolean;
  item: ShareItem | null;
  /** The shared local site, when it still exists. */
  site: Site | undefined;
  tld: string;
  /** The domain tunnel's name. */
  tunnelName: string | null;
  cloudflared: CloudflaredStatus | null;
  /** Whether the domain tunnel can be started (sites exposed, no live instance). */
  canRestart: boolean;
  /** Whether a Cloudflare account is connected; starting the domain tunnel needs one. */
  loggedIn: boolean;
  /** The view's in-flight action key, e.g. `stop:app`. */
  busy: string | null;
  connected: boolean;
}>();

const emit = defineEmits<{
  close: [];
  stop: [];
  removeFromDomain: [];
  restart: [];
  password: [];
  copy: [url: string];
  open: [url: string];
}>();

const drawer = computed(() => props.mode === "drawer");
const locked = computed(() => props.busy !== null || !props.connected);
const failed = computed(() => props.item?.state === "failed");
const reachable = computed(() => !!props.item?.url && props.item.state !== "failed" && props.item.state !== "offline");
const localHost = computed(() => `${props.item?.site ?? ""}.${props.tld}`);

const subtitle = computed(() => {
  const item = props.item;
  if (!item) return "";
  const over = `${localHost.value} over ${scheme(props.site)}`;
  if (item.kind === "quick") return `Sharing ${over} · anyone with the link can reach it`;
  return props.tunnelName ? `Exposed through tunnel ${props.tunnelName} · ${over}` : `Exposed on your domain · ${over}`;
});

function isBusy(action: string): boolean {
  return props.busy === `${action}:${props.item?.site}`;
}

const TOOL = "h-8 gap-1.5 px-3 text-[13px] font-semibold";
const CELL = "flex min-w-0 flex-col gap-1 px-[18px] py-4";
const CELL_LABEL = "text-[11px] font-medium uppercase tracking-[0.6px] text-faint";
</script>

<template>
  <InspectorFrame
    :mode="mode"
    :shown="shown"
    label="Share details"
    testid="share-inspector"
    backdrop-class="share-inspector-backdrop"
    wide-drawer
    @close="emit('close')"
  >
    <div v-if="item" :key="item.key" class="flex flex-1 flex-col px-7 pb-6 pt-6">
      <div class="flex items-start gap-3" data-testid="share-header">
        <div class="min-w-0 flex-1">
          <h2 class="flex items-center gap-2.5">
            <span class="truncate text-[22px] font-semibold leading-tight tracking-[-0.2px]">{{ item.site }}</span>
            <span
              class="shrink-0 rounded bg-brand/15 px-1.5 py-0.5 text-[11px] font-semibold uppercase tracking-[0.4px] text-brand"
              data-testid="share-chip"
            >
              {{ typeFacts(item.kind).title }}
            </span>
          </h2>
          <p class="mt-1.5 truncate text-xs text-muted-foreground" data-testid="share-subtitle">{{ subtitle }}</p>
        </div>

        <div class="flex shrink-0 items-center gap-2">
          <Button
            variant="outline"
            size="icon"
            :class="cn('size-8 shrink-0', item.protected && 'text-brand')"
            aria-label="Share password"
            :title="item.protected ? 'Password on' : 'Set a share password'"
            :disabled="locked"
            data-action="password"
            @click="emit('password')"
          >
            <Spinner v-if="isBusy('password')" class="size-4" />
            <KeyRound v-else />
          </Button>
          <Button
            v-if="item.kind === 'quick'"
            variant="outline"
            :class="TOOL"
            :disabled="locked"
            data-action="stop"
            @click="emit('stop')"
          >
            <Spinner v-if="isBusy('stop')" class="size-4" />
            <Square v-else />
            Stop sharing
          </Button>
          <template v-else>
            <Button
              v-if="canRestart"
              variant="outline"
              :class="TOOL"
              :disabled="locked || !loggedIn"
              :title="loggedIn ? undefined : 'Connect your Cloudflare account in Setup first'"
              data-action="restart"
              @click="emit('restart')"
            >
              <Spinner v-if="busy === 'named'" class="size-4" />
              <RotateCw v-else />
              {{ item.state === "offline" ? "Start tunnel" : "Restart tunnel" }}
            </Button>
            <Button
              variant="outline"
              :class="TOOL"
              :disabled="locked"
              data-action="remove"
              @click="emit('removeFromDomain')"
            >
              <Spinner v-if="isBusy('enable')" class="size-4" />
              <X v-else />
              Remove from domain
            </Button>
          </template>
          <Button
            v-if="drawer"
            variant="outline"
            size="icon"
            class="size-8 shrink-0"
            aria-label="Close share details"
            @click="emit('close')"
          >
            <X />
          </Button>
        </div>
      </div>

      <div :class="cn(CARD, 'mt-5 flex items-center gap-4 p-5')" data-testid="share-url">
        <div class="min-w-0 flex-1">
          <p :class="CELL_LABEL">Public URL</p>
          <p
            v-if="item.url"
            :class="cn('mt-1.5 truncate font-mono text-[18px]', reachable ? 'text-foreground' : 'text-muted-foreground')"
          >
            {{ item.url }}
          </p>
          <p v-else class="mt-1.5 text-[13px] text-faint">Waiting for the public URL…</p>
        </div>
        <template v-if="!failed">
          <Button
            variant="outline"
            :class="TOOL"
            :disabled="!item.url"
            data-action="copy"
            @click="item.url && emit('copy', item.url)"
          >
            <Copy /> Copy
          </Button>
          <Button
            variant="outline"
            :class="TOOL"
            :disabled="!item.url"
            data-action="open"
            @click="item.url && emit('open', item.url)"
          >
            <ExternalLink /> Open
          </Button>
        </template>
      </div>

      <div
        :class="cn(CARD, 'mt-4 grid grid-cols-3 divide-x', failed && 'divide-destructive/30 border-destructive/40 bg-destructive/[0.06]')"
        data-testid="share-facts"
        :data-failed="failed ? 'true' : undefined"
      >
        <div :class="CELL">
          <span :class="CELL_LABEL">Type</span>
          <span class="truncate text-[13px] font-semibold">{{ typeFacts(item.kind).title }}</span>
          <span class="truncate text-xs text-muted-foreground">{{ typeFacts(item.kind).detail }}</span>
        </div>
        <div :class="CELL">
          <span :class="CELL_LABEL">Local site</span>
          <span class="truncate text-[13px] font-semibold">{{ localHost }}</span>
          <span class="truncate text-xs text-muted-foreground">{{ servedOver(site) }}</span>
        </div>
        <div :class="CELL" data-testid="share-status">
          <span :class="CELL_LABEL">Status</span>
          <span class="flex items-center gap-1.5 text-[13px] font-semibold">
            <span :class="cn('size-1.5 shrink-0 rounded-full', stateDotClass(item.state))" aria-hidden="true" />
            <span :class="failed ? 'text-destructive' : undefined">{{ stateLabel(item.state) }}</span>
          </span>
          <span class="line-clamp-2 text-xs text-muted-foreground" :title="statusDetail(item, cloudflared, loggedIn)">
            {{ statusDetail(item, cloudflared, loggedIn) }}
          </span>
        </div>
      </div>

      <ul v-if="item.warnings.length" class="mt-4 flex flex-col gap-1.5" data-testid="share-warnings">
        <li v-for="w in item.warnings" :key="w" class="flex items-start gap-2 text-xs text-warning">
          <TriangleAlert class="mt-px size-3.5 shrink-0" aria-hidden="true" />
          <span>{{ w }}</span>
        </li>
      </ul>

      <div
        v-if="item.kind === 'quick'"
        class="mt-4 flex items-start gap-3 rounded-[10px] border bg-sidebar p-4 text-[13px] leading-relaxed text-muted-foreground"
        data-testid="share-note"
      >
        <Info class="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <p>
          Quick links are for development. The URL changes whenever the tunnel restarts, traffic is capped at 200
          concurrent requests, and server-sent events aren't supported. For a stable address, expose the site on
          your own domain instead.
        </p>
      </div>
    </div>
  </InspectorFrame>
</template>
