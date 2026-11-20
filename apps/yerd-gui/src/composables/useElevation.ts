import { computed, ref, watch, type ComputedRef, type Ref } from "vue";

import { useDaemon } from "@/composables/useDaemon";
import { loadPlatform, usePlatform } from "@/composables/usePlatform";
import { useToast } from "@/composables/useToast";
import {
  elevate,
  elevateAll,
  elevateResolverPorts,
  IpcError,
  trustCa,
  unelevate,
  untrustCa,
} from "@/ipc/client";
import type { ElevateTarget } from "@/ipc/types";
import { type PrivilegeItem, privilegeItems } from "@/lib/elevation";

/**
 * OS-privilege actions (Fix all, Fix one, Revert behind a confirm) over the
 * shared daemon report, shared by the Welcome screen's EnvironmentCard and the
 * Doctor screen so both run the same elevation paths.
 */

/** Copy for the Revert confirm; `note` is a secondary line under the body. */
export interface UnelevateCopy {
  title: string;
  body: string;
  note?: string;
}

const UNELEVATE_COPY: Record<ElevateTarget, UnelevateCopy> = {
  trust: {
    title: "Untrust local CA",
    body: "Removes Yerd's local CA trust for your user account. HTTPS .test sites will show certificate warnings until you trust it again.",
    note: "A trust set system-wide via `sudo yerd elevate trust` must be removed with `sudo yerd unelevate trust`.",
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

/** What `useElevation` exposes. */
export interface Elevation {
  /** In-app elevation exists on macOS and Linux only. */
  canElevate: ComputedRef<boolean>;
  /** The in-flight action, e.g. `elevate:all`, `elevate:trust`, `unelevate:ports`. */
  busy: Ref<string | null>;
  items: ComputedRef<PrivilegeItem[]>;
  anyFixable: ComputedRef<boolean>;
  fixableCount: ComputedRef<number>;
  fixAll: () => Promise<void>;
  elevateOne: (target: ElevateTarget) => Promise<void>;
  unelevateOpen: Ref<boolean>;
  unelevateCopy: ComputedRef<UnelevateCopy | null>;
  openUnelevate: (target: ElevateTarget) => void;
  /** Runs the pending revert; callers close the confirm first. */
  confirmUnelevate: () => Promise<void>;
}

/** `onSettled` runs after every elevation or revert, success or failure. */
export function useElevation(opts: { onSettled?: () => void } = {}): Elevation {
  const { report, refresh: refreshStatus } = useDaemon();
  const toast = useToast();
  const { platform, isMac } = usePlatform();
  void loadPlatform();

  const busy = ref<string | null>(null);
  const systemTrustRemains = ref(false);
  const canElevate = computed(() => platform.value === "linux" || platform.value === "macos");

  const items = computed<PrivilegeItem[]>(() =>
    report.value ? privilegeItems(report.value, isMac.value, systemTrustRemains.value) : [],
  );
  const fixableCount = computed(() => items.value.filter((i) => i.fixable).length);
  const anyFixable = computed(() => fixableCount.value > 0);

  watch(
    () => report.value?.ca.trusted_system,
    (trusted) => {
      if (trusted !== true) systemTrustRemains.value = false;
    },
  );

  /** Trust the CA: macOS uses the in-process user-domain path; elsewhere the CLI. */
  async function trustStep(): Promise<void> {
    if (isMac.value) {
      await trustCa();
      systemTrustRemains.value = false;
    } else {
      await elevate("trust");
    }
  }

  /**
   * On macOS, CA trust is in-process (user domain, no root) and prompts as
   * "Yerd"; resolver and ports both need root, so they share ONE osascript
   * prompt. The two steps run independently so cancelling the root prompt
   * doesn't mark the trust step failed.
   */
  /** The macOS Fix all steps for what still needs fixing: trust on its own
   *  (its own keychain prompt), then resolver and ports batched into one
   *  password prompt when both need it, else whichever one does. */
  function macFixAllSteps(
    needed: Set<ElevateTarget>,
  ): { label: string; run: () => Promise<unknown> }[] {
    const steps: { label: string; run: () => Promise<unknown> }[] = [];
    if (needed.has("trust")) steps.push({ label: "trust", run: () => trustStep() });
    if (needed.has("resolver") && needed.has("ports")) {
      steps.push({ label: "resolver + ports", run: () => elevateResolverPorts() });
    } else if (needed.has("resolver")) {
      steps.push({ label: "resolver", run: () => elevate("resolver") });
    } else if (needed.has("ports")) {
      steps.push({ label: "ports", run: () => elevate("ports") });
    }
    return steps;
  }

  async function fixAll(): Promise<void> {
    if (busy.value) return;
    busy.value = "elevate:all";
    try {
      if (isMac.value) {
        const steps = macFixAllSteps(new Set(items.value.filter((i) => i.fixable).map((i) => i.target)));
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
      opts.onSettled?.();
    }
  }

  async function elevateOne(target: ElevateTarget): Promise<void> {
    if (busy.value) return;
    busy.value = `elevate:${target}`;
    try {
      if (target === "trust") {
        await trustStep();
      } else {
        await elevate(target);
      }
      toast.success("Privilege granted");
    } catch (e) {
      toast.error("Elevation failed", (e as IpcError).message);
    } finally {
      busy.value = null;
      await refreshStatus();
      opts.onSettled?.();
    }
  }

  const unelevateOpen = ref(false);
  const pendingUnelevate = ref<ElevateTarget | null>(null);
  const unelevateCopy = computed(() =>
    pendingUnelevate.value ? UNELEVATE_COPY[pendingUnelevate.value] : null,
  );

  function openUnelevate(target: ElevateTarget): void {
    pendingUnelevate.value = target;
    unelevateOpen.value = true;
  }

  async function confirmUnelevate(): Promise<void> {
    if (busy.value) return;
    const target = pendingUnelevate.value;
    if (!target) return;
    busy.value = `unelevate:${target}`;
    try {
      if (target === "trust" && isMac.value) {
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
      opts.onSettled?.();
    }
  }

  return {
    canElevate,
    busy,
    items,
    anyFixable,
    fixableCount,
    fixAll,
    elevateOne,
    unelevateOpen,
    unelevateCopy,
    openUnelevate,
    confirmUnelevate,
  };
}
