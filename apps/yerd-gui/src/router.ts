import { createRouter, createWebHashHistory } from "vue-router";

// Hash history: the webview loads from a file/asset origin, so hash routing
// avoids server-rewrite assumptions.
export const router = createRouter({
  history: createWebHashHistory(),
  routes: [
    { path: "/", redirect: "/php" },
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
      path: "/about",
      name: "about",
      component: () => import("@/views/AboutView.vue"),
    },
  ],
});
