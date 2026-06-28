/**
 * Installs the single window-level keydown dispatcher for a given window scope
 * and wires the registry's injected context to the real app singletons (router,
 * theme, daemon IPC, Tauri window). Returns the palette/cheat-sheet open refs so
 * the app shell can render those overlays.
 *
 * The listener runs in the capture phase so it sees the chord before the
 * webview's own handling; on a registry match it prevents default (e.g. stops a
 * ⌘R page reload) and runs the command.
 */
import { getCurrentWindow } from "@tauri-apps/api/window";
import { onMounted, onUnmounted, ref, type Ref } from "vue";
import { useRouter } from "vue-router";

import { useToast } from "@/composables/useToast";
import { isEditable } from "@/lib/desktop";
import { setTheme } from "@/lib/theme";
import { IpcError, restartDaemon } from "@/ipc/client";

import { matchChord } from "./chord";
import { isMac } from "./platform";
import {
  buildCommands,
  commandsForScope,
  type Command,
  type ShortcutCtx,
  type WindowScope,
} from "./registry";
import { getViewActions } from "./useViewActions";

export interface UseShortcuts {
  paletteOpen: Ref<boolean>;
  cheatSheetOpen: Ref<boolean>;
  /** Commands active in this window, for the palette and cheat-sheet. */
  commands: Command[];
  /** Execute a command against the wired context. */
  run: (cmd: Command) => void;
}

export function useShortcuts(scope: WindowScope): UseShortcuts {
  const router = useRouter();
  const toast = useToast();
  const paletteOpen = ref(false);
  const cheatSheetOpen = ref(false);

  const ctx: ShortcutCtx = {
    push: (path) => void router.push(path),
    openPalette: () => (paletteOpen.value = true),
    openCheatSheet: () => (cheatSheetOpen.value = true),
    toggleTheme: () => {
      const dark = document.documentElement.classList.contains("dark");
      setTheme(dark ? "light" : "dark");
    },
    restartDaemon: async () => {
      try {
        await restartDaemon();
        toast.success("Daemon restarted");
      } catch (e) {
        toast.error("Couldn't restart the daemon", (e as IpcError).message);
      }
    },
    closeWindow: () => void getCurrentWindow().close(),
    view: getViewActions,
  };

  const mac = isMac();
  const commands = commandsForScope(buildCommands(), scope, mac);

  function onKey(e: KeyboardEvent): void {
    const editable = isEditable(e.target);
    for (const cmd of commands) {
      if (editable && !cmd.chord.mod && !cmd.chord.ctrl) continue;
      if (!matchChord(e, cmd.chord, mac)) continue;
      e.preventDefault();
      e.stopPropagation();
      cmd.run(ctx);
      return;
    }
  }

  onMounted(() => globalThis.addEventListener("keydown", onKey, { capture: true }));
  onUnmounted(() =>
    globalThis.removeEventListener("keydown", onKey, { capture: true }),
  );

  return {
    paletteOpen,
    cheatSheetOpen,
    commands,
    run: (cmd: Command) => cmd.run(ctx),
  };
}
