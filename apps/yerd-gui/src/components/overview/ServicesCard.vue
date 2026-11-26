<script setup lang="ts">
import { ArrowRight, Play, RotateCw, Square } from "lucide-vue-next";
import { computed, ref } from "vue";
import { RouterLink } from "vue-router";

import Button from "@/components/ui/Button.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { invalidate } from "@/composables/useResource";
import { restartService, startService, stopService } from "@/ipc/client";
import type { ServiceStatus, SiteEntry } from "@/ipc/types";
import { serviceSummary } from "@/lib/overview";
import { canStart, canStop, siteDomain, versionLabel } from "@/lib/services";
import { toastOnError } from "@/lib/toastActions";
import { cn, runStateDotClass } from "@/lib/utils";

/**
 * The Overview's services card: one row per installed service with its state,
 * version and port, and Restart/Stop or Start. A per-site instance (Reverb)
 * also shows its site's domain, so two instances can be told apart. Each action
 * refreshes the daemon report and the Services page's cached list.
 */
const props = defineProps<{ services: ServiceStatus[]; tld: string; sites: readonly SiteEntry[] }>();

const { refresh } = useDaemon();
const summary = computed(() => serviceSummary(props.services));
const busy = ref<string | null>(null);

type Action = "start" | "stop" | "restart";

const ACTIONS: Record<Action, { run: (id: string) => Promise<void>; failure: string }> = {
  start: { run: startService, failure: "Couldn't start" },
  stop: { run: stopService, failure: "Couldn't stop" },
  restart: { run: restartService, failure: "Couldn't restart" },
};

function domainOf(s: ServiceStatus): string {
  return siteDomain(s, props.tld, props.sites);
}

function label(s: ServiceStatus): string {
  return s.site ? `${s.display_name} ${domainOf(s)}` : s.display_name;
}

async function act(s: ServiceStatus, action: Action): Promise<void> {
  if (busy.value) return;
  busy.value = `${action}:${s.service}`;
  const { run, failure } = ACTIONS[action];
  try {
    await toastOnError(`${failure} ${label(s)}`, () => run(s.service));
    await Promise.all([refresh(), invalidate("services")]);
  } finally {
    busy.value = null;
  }
}

function portLabel(s: ServiceStatus): string {
  return s.state === "running" ? `:${s.port}` : s.state;
}

const ICON_BUTTON = "size-7 px-0 [&_svg]:size-3.5";
</script>

<template>
  <section
    data-testid="services-card"
    class="rounded-xl border bg-muted px-5 pb-3 pt-4 text-card-foreground dark:bg-card"
  >
    <div class="flex items-center gap-3 pb-2">
      <h3 class="text-[15px] font-semibold">Services</h3>
      <span v-if="summary.rows.length" class="text-xs text-faint" data-testid="services-summary">
        {{ summary.running }} of {{ summary.rows.length }} running
      </span>
      <RouterLink
        to="/services"
        class="ml-auto inline-flex shrink-0 items-center gap-1 text-[13px] font-medium text-brand hover:underline"
      >
        Manage <ArrowRight class="size-3.5" aria-hidden="true" />
      </RouterLink>
    </div>

    <p v-if="summary.rows.length === 0" class="py-3 text-[13px] text-muted-foreground">
      No services yet.
    </p>
    <ul v-else class="divide-y">
      <li
        v-for="s in summary.rows"
        :key="s.service"
        data-testid="service-row"
        :data-service="s.service"
        class="grid h-[42px] grid-cols-[minmax(0,1fr)_64px_72px_auto] items-center gap-2"
      >
        <span class="flex min-w-0 items-center gap-2.5">
          <span :class="cn('size-1.5 shrink-0 rounded-full', runStateDotClass(s.state))" aria-hidden="true" />
          <span
            :class="
              cn(
                'truncate text-[13px]',
                s.state === 'running' ? 'font-semibold' : 'text-muted-foreground',
              )
            "
          >
            {{ s.display_name }}
          </span>
          <span v-if="s.site" class="truncate font-mono text-xs text-faint" data-testid="service-site">
            {{ domainOf(s) }}
          </span>
        </span>
        <span class="truncate font-mono text-xs text-faint">{{ versionLabel(s) }}</span>
        <span class="truncate font-mono text-xs text-faint">{{ portLabel(s) }}</span>
        <span class="flex justify-end gap-1.5">
          <template v-if="canStop(s)">
            <Button
              variant="outline"
              size="sm"
              :class="ICON_BUTTON"
              :disabled="busy !== null"
              :aria-label="`Restart ${label(s)}`"
              title="Restart"
              @click="act(s, 'restart')"
            >
              <Spinner v-if="busy === `restart:${s.service}`" />
              <RotateCw v-else />
            </Button>
            <Button
              variant="outline"
              size="sm"
              :class="ICON_BUTTON"
              :disabled="busy !== null"
              :aria-label="`Stop ${label(s)}`"
              title="Stop"
              @click="act(s, 'stop')"
            >
              <Spinner v-if="busy === `stop:${s.service}`" />
              <Square v-else class="!size-3" />
            </Button>
          </template>
          <Button
            v-else-if="canStart(s)"
            variant="outline"
            size="sm"
            class="h-7 gap-1.5 px-2.5"
            :disabled="busy !== null"
            :aria-label="`Start ${label(s)}`"
            @click="act(s, 'start')"
          >
            <Spinner v-if="busy === `start:${s.service}`" class="!size-3" />
            <Play v-else class="!size-3 fill-current text-success" /> Start
          </Button>
        </span>
      </li>
    </ul>
  </section>
</template>
