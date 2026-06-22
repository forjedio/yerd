import { createRouter, createWebHashHistory } from "vue-router";

// Hash history: the webview loads from a file/asset origin, so hash routing
// avoids server-rewrite assumptions.
export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/overview" },
    {
      // The home/dashboard: a glance at the running system. Owns the daemon-down
      // hero, so it stays reachable when the socket is unreachable.
      path: "/overview",
      name: "overview",
      component: () => import("@/views/OverviewView.vue"),
    },
    {
      // Settings (the page is labelled "Settings"; the route name stays
      // "general" so the tray/links and the daemon-free set don't churn).
      path: "/general",
      name: "general",
      component: () => import("@/views/GeneralView.vue"),
    },
    {
      path: "/php",
      name: "php",
      component: () => import("@/views/PhpView.vue"),
    },
    {
      path: "/sites",
      name: "sites",
      component: () => import("@/views/SitesView.vue"),
    },
    {
      path: "/tooling",
      name: "tooling",
      component: () => import("@/views/ToolingView.vue"),
    },
    {
      path: "/services",
      name: "services",
      component: () => import("@/views/ServicesView.vue"),
    },
    {
      path: "/dumps",
      name: "dumps",
      component: () => import("@/views/LaravelDumpsView.vue"),
    },
    {
      // Standalone viewer rendered in the separate "dumps" window (no app shell).
      path: "/dumps-window",
      name: "dumps-window",
      component: () => import("@/views/DumpsWindowView.vue"),
    },
    {
      path: "/mail",
      name: "mail",
      component: () => import("@/views/MailView.vue"),
    },
    {
      // The separate "Mails" window loads this route. `standalone` tells App.vue
      // to render it bare (no sidebar/titlebar) and skip the daemon poller.
      path: "/mails-viewer",
      name: "mails-viewer",
      meta: { standalone: true },
      component: () => import("@/views/MailsViewerView.vue"),
    },
    {
      path: "/doctor",
      name: "doctor",
      component: () => import("@/views/DoctorView.vue"),
    },
    {
      path: "/about",
      name: "about",
      component: () => import("@/views/AboutView.vue"),
    },
  ],
});
