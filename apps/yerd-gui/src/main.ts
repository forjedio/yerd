import { createApp } from "vue";

import App from "./App.vue";
import { router } from "./router";
import { initDesktopChrome } from "./lib/desktop";
import { log } from "./lib/log";
import { initTheme } from "./lib/theme";
import "./style.css";

// Follow the OS light/dark preference, and behave like a native window.
initTheme();
initDesktopChrome();

// Funnel uncaught frontend errors into the GUI session log so they show up in
// About → GUI Logs. A reentrancy guard keeps a logging failure from looping back
// through `unhandledrejection`.
let inErrorHandler = false;
function logUncaught(prefix: string, detail: unknown): void {
  if (inErrorHandler) return;
  inErrorHandler = true;
  try {
    const msg = detail instanceof Error ? `${detail.message}\n${detail.stack ?? ""}` : String(detail);
    log.error(`${prefix}: ${msg}`);
  } finally {
    inErrorHandler = false;
  }
}
window.addEventListener("error", (e) => logUncaught("uncaught error", e.error ?? e.message));
window.addEventListener("unhandledrejection", (e) => logUncaught("unhandled rejection", e.reason));

const app = createApp(App);
app.config.errorHandler = (err) => logUncaught("vue error", err);
app.use(router).mount("#app");
