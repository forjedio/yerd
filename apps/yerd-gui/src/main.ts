import { createApp } from "vue";

import App from "./App.vue";
import { router } from "./router";
import { initDesktopChrome } from "./lib/desktop";
import { initTheme } from "./lib/theme";
import "./style.css";

// Follow the OS light/dark preference, and behave like a native window.
initTheme();
initDesktopChrome();

createApp(App).use(router).mount("#app");
