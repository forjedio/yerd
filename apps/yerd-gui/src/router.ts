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
      path: "/laravel/dumps",
      name: "laravel-dumps",
      component: () => import("@/views/LaravelDumpsView.vue"),
    },
    {
      // Standalone viewer rendered in the separate "dumps" window (no app shell).
      path: "/dumps-window",
      name: "dumps-window",
      component: () => import("@/views/DumpsWindowView.vue"),
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
