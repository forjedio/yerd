import type { LaravelOptions } from "@/ipc/types";

/**
 * The `laravel new` command a Laravel create job will run, for the New site
 * dialog's "What Yerd will do" strip. A display-only mirror of
 * `bin/yerdd/src/create_site/laravel.rs::build_new_args`: same flags in the same
 * order, minus the `--no-interaction`, `--no-ansi` and `--no-boost` noise. The
 * daemon builds its own argv and never reads this string.
 */
export function laravelCommand(name: string, o: LaravelOptions): string {
  const args = ["laravel", "new", name];
  if (typeof o.starter_kit === "object") args.push(`--using=${o.starter_kit.community}`);
  else if (o.starter_kit !== "none") args.push(`--${o.starter_kit}`);
  if (o.auth === "work_os") args.push("--workos");
  if (o.livewire_class_components) args.push("--livewire-class-components");
  if (o.teams) args.push("--teams");
  args.push(o.testing === "pest" ? "--pest" : "--phpunit");
  args.push(`--database=${o.database}`);
  if (o.js !== "skip") args.push(`--${o.js}`);
  if (o.git) args.push("--git");
  if (o.boost) args.push("--boost");
  return args.join(" ");
}

/** An elapsed time as the Creating header's clock: `0:42`, `12:05`. */
export function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes}:${String(seconds).padStart(2, "0")}`;
}

/** A finished job's duration for the Done summary: `42s`, `1m 12s`, `3m`. */
export function formatDuration(ms: number): string {
  const total = Math.max(0, Math.round(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  if (minutes === 0) return `${seconds}s`;
  return seconds === 0 ? `${minutes}m` : `${minutes}m ${seconds}s`;
}
