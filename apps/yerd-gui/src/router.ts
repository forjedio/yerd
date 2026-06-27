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
    // `title`/`subtitle` mirror each view's own PageHeader so the daemon-down
    // screen (rendered by AppShell when the socket is unreachable) shows the same
    // page header - without the view's action buttons.
    {
      path: "/php",
      name: "php",
      meta: { title: "PHP", subtitle: "Installed versions, updates, and the global default" },
      component: () => import("@/views/PhpView.vue"),
    },
    {
      path: "/sites",
      name: "sites",
      meta: { title: "Sites", subtitle: "Parked and linked .test sites" },
      component: () => import("@/views/SitesView.vue"),
    },
    {
      path: "/tooling",
      name: "tooling",
      meta: {
        title: "Tooling",
        subtitle:
          "Install developer tools - bundled, self-contained, and added to your PATH alongside PHP.",
      },
      component: () => import("@/views/ToolingView.vue"),
    },
    {
      path: "/services",
      name: "services",
      meta: { title: "Services", subtitle: "Databases and caches Yerd supervises" },
      component: () => import("@/views/ServicesView.vue"),
    },
    {
      path: "/dumps",
      name: "dumps",
      meta: { title: "Dumps", subtitle: "Intercept dump() calls and Laravel telemetry" },
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
      meta: { title: "Mail", subtitle: "Capture and inspect emails your apps send during development" },
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
      meta: { title: "Doctor", subtitle: "Health checks and safe one-click fixes" },
      component: () => import("@/views/DoctorView.vue"),
    },
    {
      path: "/about",
      name: "about",
      component: () => import("@/views/AboutView.vue"),
    },
  ],
});
