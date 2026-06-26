<script setup lang="ts">
import { Undo2 } from "lucide-vue-next";
import { computed, onMounted, ref, watch } from "vue";

import ComingSoon from "@/components/ComingSoon.vue";
import StatusPill, { type Tone } from "@/components/StatusPill.vue";
import Button from "@/components/ui/Button.vue";
import Card from "@/components/ui/Card.vue";
import CardContent from "@/components/ui/CardContent.vue";
import CardDescription from "@/components/ui/CardDescription.vue";
import CardHeader from "@/components/ui/CardHeader.vue";
import CardTitle from "@/components/ui/CardTitle.vue";
import Modal from "@/components/ui/Modal.vue";
import Spinner from "@/components/ui/Spinner.vue";
import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  elevate,
  elevateAll,
  elevateResolverPorts,
  hostPlatform,
  IpcError,
  trustCa,
  unelevate,
  untrustCa,
} from "@/ipc/client";
import type { ElevateTarget, StatusReport } from "@/ipc/types";

// Self-contained OS-privileges panel (CA trust, .test resolver, privileged
// ports). Lives on the Doctor page alongside the health checks - it's the same
// "diagnose and fix" job. Owns its own busy state and platform probe; reads the
// shared daemon report and refreshes it after any elevation.
const { connected, report, refresh: refreshStatus } = useDaemon();
const toast = useToast();

// Emitted after any elevation/revert completes (success or failure), so a parent
// (e.g. the Doctor page) can re-run dependent checks like the health table.
const emit = defineEmits<{ elevated: [] }>();

const busy = ref<string | null>(null);
const platform = ref<string>("");
// macOS only: set true when a GUI untrust left a system-wide trust (set via
// `sudo yerd elevate trust`) in place - the GUI can't remove that without root.
// Drives the trust row to hide the (now-useless) Revert button and show guidance.
// Cleared once the CA actually reads not-trusted again - see the watcher below.
const systemTrustRemains = ref(false);
const canElevate = computed(
  () => platform.value === "linux" || platform.value === "macos",
);

// ── tri-state OS privileges ──
type Tri = boolean | null;
function triTone(v: Tri): Tone {
  if (v === true) return "ok";
  if (v === false) return "bad";
  return "unknown";
}
function triLabel(v: Tri, yes: string, no: string): string {
  if (v === true) return yes;
  if (v === false) return no;
  return "unknown";
}

const PRIVILEGED_PORT_CEILING = 1024;
function privilegedFallback(r: StatusReport): boolean {
  return (
    (r.http.requested < PRIVILEGED_PORT_CEILING && r.http.fell_back) ||
    (r.https.requested < PRIVILEGED_PORT_CEILING && r.https.fell_back)
  );
}
function portsElevated(r: StatusReport): boolean {
  return !privilegedFallback(r) || r.port_redirect === true;
}

interface EnvItem {
  key: string;
  label: string;
  value: Tri;
  fixable: boolean;
  // Shown only when *Yerd* established this privilege (so the undo is meaningful).
  // Never derived from `portsElevated` (true even when Yerd installed nothing).
  // Ports is reversible on macOS only (Linux setcap has no clean reverse).
  unelevatable: boolean;
  target: ElevateTarget;
  yes: string;
  no: string;
  // Optional inline guidance shown in the action cell (e.g. when a residual
  // system-wide trust can only be removed from a terminal).
  note?: string;
}
const envItems = computed<EnvItem[]>(() => {
  const r = report.value;
  if (!r) return [];
  const mac = platform.value === "macos";
  return [
    {
      key: "trust",
      label: "Local CA trusted",
      value: r.ca.trusted_system,
      fixable: r.ca.trusted_system !== true,
      // Hide Revert when a system-wide trust remains that the GUI can't undo -
      // a lingering Revert there does nothing and looks broken.
      unelevatable: r.ca.trusted_system === true && !(mac && systemTrustRemains.value),
      target: "trust",
      yes: "trusted",
      no: "not trusted",
      note:
        mac && systemTrustRemains.value
          ? "Trusted system-wide (via Terminal) - run `sudo yerd unelevate trust` to remove it."
          : undefined,
    },
    {
      key: "resolver",
      label: ".test resolver installed",
      value: r.resolver_installed,
      fixable: r.resolver_installed !== true,
      unelevatable: r.resolver_installed === true,
      target: "resolver",
      yes: "installed",
      no: "not installed",
    },
    {
      key: "ports",
      label: "Privileged ports (80/443)",
      // Degraded: the daemon bound no web ports. On macOS elevation can't help
      // yet (the pf redirect would point at the unbound ports, and the CLI
      // refuses), so withhold the fix until working ports are set. On Linux the
      // ports fix is `setcap`, which binds 80/443 DIRECTLY and doesn't depend on
      // the fallback ports - it's the correct degraded recovery, so keep it
      // offered there.
      value: r.web_unbound ? false : portsElevated(r),
      fixable: r.web_unbound ? !mac : !portsElevated(r),
      unelevatable: r.port_redirect === true && mac,
      target: "ports",
      yes: r.port_redirect === true && privilegedFallback(r) ? "redirected" : "bound",
      no: r.web_unbound
        ? mac
          ? "not serving - set working ports first"
          : "not serving - elevate to bind 80/443"
        : "fell back to high ports",
      note:
        r.web_unbound && mac
          ? "Yerd couldn't bind its web ports. Set working ports in Settings, then elevate."
          : undefined,
    },
  ];
});

onMounted(() => {
  hostPlatform().then((p) => (platform.value = p));
});

// Once the CA actually reads not-trusted again (e.g. the user removed the
// system-wide trust via `sudo yerd unelevate trust`), drop the residual flag so
// the trust row returns to its normal Fix/Revert behaviour.
watch(
  () => report.value?.ca.trusted_system,
  (trusted) => {
    if (trusted !== true) systemTrustRemains.value = false;
  },
);

// ── elevation ──
// Anything still needing a privileged fix? Gates the "Fix all" button.
const anyFixable = computed(() => envItems.value.some((i) => i.fixable));

async function onFixAll(): Promise<void> {
  if (busy.value) return;
  busy.value = "elevate:all";
  try {
    if (platform.value === "macos") {
      // On macOS, CA trust is in-process (user domain, no root) and prompts as
      // "Yerd"; resolver + ports both need root, so batch them into ONE osascript
      // prompt (instead of one each). Two steps, run independently so cancelling
      // the root prompt doesn't mark the trust step failed.
      const steps: { label: string; run: () => Promise<unknown> }[] = [
        { label: "trust", run: () => onTrustCa() },
        { label: "resolver + ports", run: () => elevateResolverPorts() },
      ];
      const failed: string[] = [];
      for (const s of steps) {
        try {
          await s.run();
        } catch (e) {
          failed.push(`${s.label}: ${(e as IpcError).message}`);
        }
      }
      if (failed.length === 0) {
        toast.success("Privileges granted");
      } else if (failed.length === steps.length) {
        toast.error("Elevation failed", failed.join("; "));
      } else {
        toast.info("Partly done", `Couldn't complete - ${failed.join("; ")}`);
      }
    } else {
      await elevateAll();
      toast.success("Privileges granted");
    }
  } catch (e) {
    toast.error("Elevation failed", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refreshStatus();
    emit("elevated");
  }
}

/** Trust the CA: macOS uses the in-process user-domain path; elsewhere the CLI. */
async function onTrustCa(): Promise<void> {
  if (platform.value === "macos") {
    await trustCa();
    systemTrustRemains.value = false;
  } else {
    await elevate("trust");
  }
}

async function onElevate(target: ElevateTarget): Promise<void> {
  if (busy.value) return;
  busy.value = `elevate:${target}`;
  try {
    if (target === "trust") {
      await onTrustCa();
    } else {
      await elevate(target);
    }
    toast.success("Privilege granted");
  } catch (e) {
    toast.error("Elevation failed", (e as IpcError).message);
  } finally {
    busy.value = null;
    await refreshStatus();
    emit("elevated");
  }
}

const unelevateOpen = ref(false);
const pendingUnelevate = ref<ElevateTarget | null>(null);
const UNELEVATE_COPY: Record<ElevateTarget, { title: string; body: string }> = {
  trust: {
    title: "Untrust local CA",
    body: "Removes Yerd's local CA trust for your user account. HTTPS .test sites will show certificate warnings until you trust it again. (A trust set system-wide via `sudo yerd elevate trust` must be removed with `sudo yerd unelevate trust`.)",
  },
  resolver: {
    title: "Remove .test resolver",
    body: "Removes Yerd's .test resolver. If a previous resolver was backed up when Yerd was set up, it's restored automatically; otherwise .test names stop resolving through Yerd.",
  },
  ports: {
    title: "Remove port redirect",
    body: "Removes the 80/443 → Yerd redirect. Sites stay reachable on Yerd's high ports until you re-enable it.",
  },
};
const unelevateCopy = computed(() =>
  pendingUnelevate.value ? UNELEVATE_COPY[pendingUnelevate.value] : null,
);
function openUnelevate(target: ElevateTarget): void {
  pendingUnelevate.value = target;
  unelevateOpen.value = true;
}
async function confirmUnelevate(close: () => void): Promise<void> {
  if (busy.value) return;
  const target = pendingUnelevate.value;
  if (!target) return;
  busy.value = `unelevate:${target}`;
  close();
  try {
    if (target === "trust" && platform.value === "macos") {
      const residual = await untrustCa();
      systemTrustRemains.value = residual;
      if (residual) {
        toast.info(
          "Removed for your user",
          "A system-wide trust set via Terminal remains - run `sudo yerd unelevate trust` to remove it.",
        );
      } else {
        toast.success("Reverted");
      }
    } else {
      await unelevate(target);
      toast.success("Reverted");
    }
  } catch (e) {
    toast.error("Couldn't revert", (e as IpcError).message);
  } finally {
    busy.value = null;
    pendingUnelevate.value = null;
    await refreshStatus();
    emit("elevated");
  }
}
</script>

<template>
  <Card>
    <CardHeader class="flex-row items-center justify-between space-y-0">
      <div class="space-y-1.5">
        <CardTitle>Environment</CardTitle>
        <CardDescription>
          OS-level trust, resolver, and privileged-port configuration.
        </CardDescription>
      </div>
      <Button
        v-if="canElevate"
        size="sm"
        :disabled="!anyFixable || busy !== null"
        @click="onFixAll"
      >
        <Spinner v-if="busy === 'elevate:all'" class="size-4" />
        Fix all (elevate)
      </Button>
    </CardHeader>
    <CardContent>
      <p
        v-if="!report && connected === false"
        class="py-6 text-center text-sm text-muted-foreground"
      >
        Start the daemon to view and change OS privileges.
      </p>
      <div v-else-if="!report" class="flex justify-center py-8"><Spinner class="size-5" /></div>
      <table v-else class="w-full text-sm">
        <thead class="sr-only">
          <tr>
            <th scope="col">Privilege</th>
            <th scope="col">Status</th>
            <th scope="col">Action</th>
          </tr>
        </thead>
        <tbody>
          <tr v-for="item in envItems" :key="item.key" class="border-b last:border-0">
            <td class="py-3 pr-4 font-medium">{{ item.label }}</td>
            <td class="py-3 pr-4">
              <StatusPill :tone="triTone(item.value)" :label="triLabel(item.value, item.yes, item.no)" />
            </td>
            <td class="py-3 pl-4 text-right">
              <template v-if="item.fixable">
                <Button
                  v-if="canElevate"
                  variant="outline"
                  size="sm"
                  :disabled="busy !== null"
                  @click="onElevate(item.target)"
                >
                  <Spinner v-if="busy === `elevate:${item.target}`" class="size-4" />
                  Fix (elevate)
                </Button>
                <ComingSoon
                  v-else
                  reason="In-app elevation isn't available on this platform yet - use `yerd elevate` in a terminal for now."
                  pill
                >
                  Fix
                </ComingSoon>
              </template>
              <Button
                v-else-if="item.unelevatable && canElevate"
                variant="ghost"
                size="sm"
                :disabled="busy !== null"
                @click="openUnelevate(item.target)"
              >
                <Spinner v-if="busy === `unelevate:${item.target}`" class="size-4" />
                <Undo2 v-else class="size-4" />
                Revert
              </Button>
              <span
                v-else-if="item.note"
                class="text-xs text-muted-foreground"
              >{{ item.note }}</span>
            </td>
          </tr>
        </tbody>
      </table>
      <p v-if="report" class="mt-3 text-xs text-muted-foreground">
        Fixes run the audited <code>yerd elevate</code> helper under an OS
        prompt; the GUI itself never runs as root.
      </p>
    </CardContent>
  </Card>

  <Modal v-model:open="unelevateOpen" :title="unelevateCopy?.title ?? 'Revert privilege'">
    <p class="text-sm text-muted-foreground">{{ unelevateCopy?.body }}</p>
    <template #footer="{ close }">
      <Button variant="ghost" @click="close">Cancel</Button>
      <Button @click="confirmUnelevate(close)">Revert</Button>
    </template>
  </Modal>
</template>
