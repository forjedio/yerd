import { createRouter, createWebHashHistory } from "vue-router";

// Hash history: the webview loads from a file/asset origin, so hash routing
// avoids server-rewrite assumptions.
export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/general" },
    {
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
      path: "/services",
      name: "services",
      component: () => import("@/views/ServicesView.vue"),
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
