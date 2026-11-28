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
    // `title` mirrors each view's own PageHeader so the daemon-down screen
    // (rendered by AppShell when the socket is unreachable) shows the same
    // toolbar title - without the view's action buttons.
    {
      path: "/php",
      name: "php",
      meta: { title: "PHP" },
      component: () => import("@/views/PhpView.vue"),
    },
    {
      path: "/node",
      name: "node",
      meta: { title: "Node" },
      component: () => import("@/views/NodeView.vue"),
    },
    {
      path: "/sites",
      name: "sites",
      meta: { title: "Sites" },
      component: () => import("@/views/SitesView.vue"),
    },
    {
      path: "/tooling",
      name: "tooling",
      meta: { title: "Tooling" },
      component: () => import("@/views/ToolingView.vue"),
    },
    {
      path: "/services",
      name: "services",
      meta: { title: "Services" },
      component: () => import("@/views/ServicesView.vue"),
    },
    {
      path: "/proxies",
      name: "proxies",
      meta: { title: "Proxies" },
      component: () => import("@/views/ProxiesView.vue"),
    },
    // The Dumps page became Insights; old links and bookmarks land there.
    { path: "/dumps", redirect: "/insights" },
    {
      path: "/insights",
      name: "insights",
      meta: { title: "Insights" },
      component: () => import("@/views/InsightsView.vue"),
    },
    {
      path: "/integrations",
      name: "integrations",
      meta: { title: "Share" },
      component: () => import("@/views/IntegrationsView.vue"),
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
      meta: { title: "Mail" },
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
      meta: { title: "Doctor" },
      component: () => import("@/views/DoctorView.vue"),
    },
    // The About page moved into Settings; the tray's About and "Update…" items
    // still navigate to /about, so land them on Settings.
    { path: "/about", redirect: "/general" },
  ],
});
