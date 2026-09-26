import { computed, nextTick, ref, type UnwrapNestedRefs } from "vue";

import { useDaemon } from "@/composables/useDaemon";
import { useToast } from "@/composables/useToast";
import {
  availablePhp,
  installPhpWithProgress,
  installToolStreamed,
  IpcError,
  listTools,
  pollJobToEnd,
} from "@/ipc/client";
import type { ToolStatus } from "@/ipc/types";

/** One row of a create wizard's prerequisites gate. */
export interface PrereqRow {
  id: string;
  label: string;
  sub: string;
  ok: boolean;
  install: () => Promise<boolean>;
  /** Set when the row can't be installed yet; disables Install and becomes its tooltip. */
  blockedReason?: string;
}

/**
 * The toolchain a create wizard needs before it unlocks: which tools are
 * present, plus streamed installs of the missing ones. `isOpen` stops an
 * install's progress poll once the wizard closes (the install itself carries on
 * in the daemon).
 */
export function useToolPrereqs(isOpen: () => boolean) {
  const toast = useToast();
  const { refresh: refreshDaemon } = useDaemon();

  const tools = ref<ToolStatus[]>([]);
  const loading = ref(false);
  const installingTool = ref<string | null>(null);
  const installingAll = ref(false);
  const installLog = ref<string[]>([]);
  const busy = computed(() => installingAll.value || installingTool.value !== null);

  /** Yerd-managed or found on the user's PATH: either can run the scaffold. */
  function available(id: string): boolean {
    return tools.value.some((t) => t.id === id && (t.installed || t.external));
  }

  /** Yerd-managed only, e.g. the Composer that builds the Laravel installer and WP-CLI. */
  function managed(id: string): boolean {
    return tools.value.some((t) => t.id === id && t.installed);
  }

  async function refresh(): Promise<void> {
    loading.value = true;
    try {
      tools.value = await listTools();
    } catch {
      tools.value = [];
    } finally {
      loading.value = false;
    }
  }

  function appendLog(lines: string[]): void {
    installLog.value.push(...lines);
  }

  /**
   * Install a managed tool, streaming its output into the install log. A job
   * still `running` when the poll returns means the wizard closed, not a failure.
   */
  async function installTool(id: string): Promise<boolean> {
    installingTool.value = id;
    installLog.value = [];
    try {
      const jobId = await installToolStreamed(id);
      const final = await pollJobToEnd(jobId, appendLog, isOpen);
      await refresh();
      if (final.state === "running") return false;
      if (final.state !== "succeeded") {
        toast.error(`Couldn't install ${id}`, final.error ?? "install failed");
        return false;
      }
      return true;
    } catch (e) {
      toast.error(`Couldn't install ${id}`, (e as IpcError).message);
      return false;
    } finally {
      installingTool.value = null;
    }
  }

  /**
   * Install the newest installable PHP that `accept` allows (the distribution
   * lists them ascending), rather than pinning a version that rots each release.
   */
  async function installPhp(accept: (v: string) => boolean, noneMessage: string): Promise<boolean> {
    installingTool.value = "php";
    installLog.value = [];
    try {
      const { available: versions } = await availablePhp();
      const accepted = versions.filter(accept);
      const version = accepted[accepted.length - 1];
      if (!version) {
        toast.error("Couldn't install PHP", noneMessage);
        return false;
      }
      appendLog([`Installing PHP ${version}…`]);
      await installPhpWithProgress(version, appendLog);
      await refreshDaemon();
      appendLog([`Installed PHP ${version}`]);
      return true;
    } catch (e) {
      toast.error("Couldn't install PHP", (e as IpcError).message);
      return false;
    } finally {
      installingTool.value = null;
    }
  }

  /**
   * Run a wizard's install-everything sequence. "Toolchain ready" is announced
   * whenever `run` reports success rather than gated on the wizard unlocking,
   * since a freshly installed PHP reaches the wizard's props asynchronously.
   */
  async function installAll(run: () => Promise<boolean>): Promise<void> {
    installingAll.value = true;
    try {
      if (!(await run())) return;
      await nextTick();
      toast.success("Toolchain ready");
    } finally {
      installingAll.value = false;
    }
  }

  function reset(): void {
    installLog.value = [];
    installingTool.value = null;
    installingAll.value = false;
  }

  return {
    tools,
    loading,
    installingTool,
    installingAll,
    installLog,
    busy,
    available,
    managed,
    refresh,
    installTool,
    installPhp,
    installAll,
    reset,
  };
}

/** `useToolPrereqs` as a wizard holds it (wrapped in `reactive`) and hands it to the shell. */
export type ToolPrereqs = UnwrapNestedRefs<ReturnType<typeof useToolPrereqs>>;
