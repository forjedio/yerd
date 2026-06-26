/**
 * Fire-and-forget frontend logger. Pushes a line into the GUI host's per-session
 * log file (`{cache}/yerd-gui.log`) via the `gui_log` command, so the About →
 * GUI Logs dialog shows frontend phase transitions and caught errors interleaved
 * with the Rust-side daemon install/upgrade/start trail.
 *
 * Every call is best-effort and MUST never throw: we use `.catch()` (not
 * try/catch) so an un-awaited `invoke` rejection can't re-enter the global
 * `unhandledrejection` handler and recurse.
 */
import { invoke } from "@tauri-apps/api/core";

export type LogLevel = "debug" | "info" | "warn" | "error";

function emit(level: LogLevel, message: string): void {
  // Swallow everything: logging failures are never worth surfacing, and a throw
  // here (especially from inside an error handler) could loop.
  void invoke("gui_log", { level, message }).catch(() => {});
}

export const log = {
  debug: (message: string): void => emit("debug", message),
  info: (message: string): void => emit("info", message),
  warn: (message: string): void => emit("warn", message),
  error: (message: string): void => emit("error", message),
};
