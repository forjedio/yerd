<script setup lang="ts">
import { ChevronDown, Copy, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";

import Button from "@/components/ui/Button.vue";
import type { DaemonDiagnostics } from "@/ipc/types";
import { copyWithToast } from "@/lib/toastActions";

// Shown when a daemon start attempt fails to connect. Leads with the
// host-computed `hints` (the actionable verdict), then an expandable raw section
// (service-manager status + log tail) for support, plus a Copy button.
const props = defineProps<{ diagnostics: DaemonDiagnostics }>();

const hasRaw = computed(
  () =>
    !!props.diagnostics.serviceStatus ||
    props.diagnostics.logTail.length > 0 ||
    props.diagnostics.spawnLogTail.length > 0 ||
    props.diagnostics.repairLogTail.length > 0,
);

/** The last two lines of this attempt's spawn log, shown inline as the likely
 *  cause. The spawn log is truncated on every start, so it belongs to this run;
 *  the rolling daemon log may still end with a previous run's lines, so it stays
 *  in the technical details rather than reading as the cause. */
const excerpt = computed(() => props.diagnostics.spawnLogTail.slice(-2));

/** A flat, paste-able plain-text dump of the whole diagnostics struct. */
function asText(): string {
  const d = props.diagnostics;
  const lines = [
    "Yerd daemon diagnostics",
    "=======================",
    `service manager : ${d.serviceManager || "(unknown)"}`,
    `socket          : ${d.socketPath} (${d.socketResponding ? "responding" : "not responding"})`,
    `yerdd binary    : ${d.yerddPath ?? "(not found)"}`,
    `translocated    : ${d.translocated}`,
    `pending approval: ${d.pendingApproval}`,
    `log file        : ${d.logPath ?? "(none)"}`,
  ];
  if (d.startError) lines.push(`start error     : ${d.startError}`);
  if (d.lastConnectError) lines.push(`connect error   : ${d.lastConnectError}`);
  if (d.hints.length) lines.push("", "Hints:", ...d.hints.map((h) => `  - ${h}`));
  if (d.serviceStatus) lines.push("", "Service status:", d.serviceStatus);
  if (d.logTail.length) lines.push("", "Daemon log (tail):", ...d.logTail);
  if (d.spawnLogTail.length) lines.push("", "Spawn log (tail):", ...d.spawnLogTail);
  if (d.repairLogTail.length)
    lines.push("", "Daemon re-registration log (tail):", ...d.repairLogTail);
  return lines.join("\n");
}

async function copy(): Promise<void> {
  await copyWithToast(asText(), {
    title: "Copied diagnostics",
    detail: "Paste this when reporting the problem.",
  });
}
</script>

<template>
  <div class="rounded-[10px] border border-destructive/40 bg-destructive/5 p-4 text-[13px]">
    <div class="flex items-center gap-2">
      <TriangleAlert class="size-4 shrink-0 text-destructive" aria-hidden="true" />
      <p class="font-semibold">The daemon didn't come up</p>
    </div>

    <ul
      v-if="diagnostics.hints.length"
      class="mt-2 list-disc space-y-1 pl-5 text-muted-foreground"
    >
      <li v-for="(hint, i) in diagnostics.hints" :key="i">{{ hint }}</li>
    </ul>
    <p v-else class="mt-2 text-muted-foreground">
      We couldn't determine the cause automatically. Open the details below
      and copy them when reporting the problem.
    </p>

    <div
      v-if="excerpt.length"
      data-testid="diagnostics-excerpt"
      class="mt-3 rounded-md bg-card px-3 py-2.5 font-mono text-xs dark:bg-background"
    >
      <p v-for="(line, i) in excerpt" :key="i" class="truncate text-destructive" :title="line">
        {{ line }}
      </p>
      <p class="mt-0.5 truncate text-faint">Spawn log</p>
    </div>

    <div class="mt-3 flow-root">
      <Button variant="outline" size="sm" class="float-left mr-2" @click="copy">
        <Copy /> Copy diagnostics
      </Button>

      <details v-if="hasRaw" class="group">
        <summary
          class="cursor-pointer list-none select-none focus-visible:outline-none [&::-webkit-details-marker]:hidden [&:focus-visible>span]:ring-1 [&:focus-visible>span]:ring-ring"
        >
          <span
            class="inline-flex h-8 items-center gap-1.5 rounded-md px-3 text-xs font-medium text-muted-foreground transition-colors hover:bg-accent hover:text-accent-foreground"
          >
            Show technical details
            <ChevronDown
              class="size-3.5 transition-transform group-open:rotate-180"
              aria-hidden="true"
            />
          </span>
        </summary>
        <div class="clear-left space-y-3 pt-3">
          <div v-if="diagnostics.serviceStatus">
            <p class="text-xs font-medium text-muted-foreground">Service status</p>
            <pre
              class="mt-1 max-h-40 overflow-auto rounded-md bg-card p-2 font-mono text-xs dark:bg-background"
            >{{ diagnostics.serviceStatus }}</pre>
          </div>
          <div v-if="diagnostics.logTail.length">
            <p class="text-xs font-medium text-muted-foreground">
              Daemon log - {{ diagnostics.logPath }}
            </p>
            <pre
              class="mt-1 max-h-40 overflow-auto rounded-md bg-card p-2 font-mono text-xs dark:bg-background"
            >{{ diagnostics.logTail.join("\n") }}</pre>
          </div>
          <div v-if="diagnostics.spawnLogTail.length">
            <p class="text-xs font-medium text-muted-foreground">Spawn log</p>
            <pre
              class="mt-1 max-h-40 overflow-auto rounded-md bg-card p-2 font-mono text-xs dark:bg-background"
            >{{ diagnostics.spawnLogTail.join("\n") }}</pre>
          </div>
          <div v-if="diagnostics.repairLogTail.length">
            <p class="text-xs font-medium text-muted-foreground">
              Daemon re-registration log
            </p>
            <pre
              class="mt-1 max-h-40 overflow-auto rounded-md bg-card p-2 font-mono text-xs dark:bg-background"
            >{{ diagnostics.repairLogTail.join("\n") }}</pre>
          </div>
        </div>
      </details>
    </div>
  </div>
</template>
