<script setup lang="ts">
import { Copy, TriangleAlert } from "lucide-vue-next";
import { computed } from "vue";

import Button from "@/components/ui/Button.vue";
import { useToast } from "@/composables/useToast";
import type { DaemonDiagnostics } from "@/ipc/types";

// Shown when a daemon start attempt fails to connect. Leads with the
// host-computed `hints` (the actionable verdict), then an expandable raw section
// (service-manager status + log tail) for support, plus a Copy button.
const props = defineProps<{ diagnostics: DaemonDiagnostics }>();

const toast = useToast();

const hasRaw = computed(
  () =>
    !!props.diagnostics.serviceStatus ||
    props.diagnostics.logTail.length > 0 ||
    props.diagnostics.spawnLogTail.length > 0,
);

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
  return lines.join("\n");
}

async function copy(): Promise<void> {
  try {
    await navigator.clipboard.writeText(asText());
    toast.success("Copied diagnostics", "Paste this when reporting the problem.");
  } catch {
    toast.error("Couldn't copy", "Your browser blocked clipboard access.");
  }
}
</script>

<template>
  <div class="rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm">
    <div class="flex items-start gap-2">
      <TriangleAlert class="mt-0.5 size-4 shrink-0 text-destructive" />
      <div class="min-w-0 flex-1">
        <p class="font-medium">The daemon didn't come up</p>

        <!-- Actionable verdict first. -->
        <ul
          v-if="diagnostics.hints.length"
          class="mt-1 list-disc space-y-1 pl-4 text-muted-foreground"
        >
          <li v-for="(hint, i) in diagnostics.hints" :key="i">{{ hint }}</li>
        </ul>
        <p v-else class="mt-1 text-muted-foreground">
          We couldn't determine the cause automatically. Open the details below
          and copy them when reporting the problem.
        </p>

        <!-- Raw details for support. -->
        <details v-if="hasRaw" class="mt-2">
          <summary class="cursor-pointer select-none text-xs text-muted-foreground">
            Show technical details
          </summary>
          <div class="mt-2 space-y-3">
            <div v-if="diagnostics.serviceStatus">
              <p class="text-xs font-medium text-muted-foreground">Service status</p>
              <pre
                class="mt-1 max-h-40 overflow-auto rounded bg-muted p-2 font-mono text-xs"
              >{{ diagnostics.serviceStatus }}</pre>
            </div>
            <div v-if="diagnostics.logTail.length">
              <p class="text-xs font-medium text-muted-foreground">
                Daemon log — {{ diagnostics.logPath }}
              </p>
              <pre
                class="mt-1 max-h-40 overflow-auto rounded bg-muted p-2 font-mono text-xs"
              >{{ diagnostics.logTail.join("\n") }}</pre>
            </div>
            <div v-if="diagnostics.spawnLogTail.length">
              <p class="text-xs font-medium text-muted-foreground">Spawn log</p>
              <pre
                class="mt-1 max-h-40 overflow-auto rounded bg-muted p-2 font-mono text-xs"
              >{{ diagnostics.spawnLogTail.join("\n") }}</pre>
            </div>
          </div>
        </details>

        <Button variant="outline" size="sm" class="mt-2" @click="copy">
          <Copy class="size-4" /> Copy diagnostics
        </Button>
      </div>
    </div>
  </div>
</template>
