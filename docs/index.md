---
layout: home

# A descriptive <title> for the homepage (the default would just be "Yerd").
title: Yerd - Local PHP development environment for macOS & Linux
titleTemplate: false

hero:
  name: Yerd
  text: Local PHP, without the friction.
  tagline: Serve your projects on .test domains over HTTP and HTTPS, run a different PHP version per site, and manage it all from one tiny daemon. No Docker, no sudo for everyday work, no subscription.
  image:
    src: /images/overview-dark.png
    alt: The Yerd desktop app
  actions:
    - theme: brand
      text: Get Started
      link: /guide/getting-started
    - theme: alt
      text: Why Yerd?
      link: /guide/introduction
    - theme: alt
      text: View on GitHub
      link: https://github.com/forjedio/yerd

features:
  - icon: 🚀
    title: Zero-config sites
    details: Drop a project into a parked directory and it's instantly live at <name>.test. Park a whole folder or link a single project under a name you choose.
  - icon: 🔒
    title: Automatic HTTPS
    details: A local certificate authority issues a per-site certificate on demand. No mkcert dance, no OpenSSL, no browser warnings once trusted - just a green padlock.
  - icon: 🐘
    title: Per-site PHP
    details: Install multiple PHP versions and pin each site to the one it needs. Set a global default, then override individual sites with a single command.
  - icon: 🪶
    title: Lightweight & native
    details: A single ~8 MB daemon binary written in Rust. No containers, no VM, no Electron - just native processes managed for you.
  - icon: 🛡️
    title: Rootless by design
    details: Setup elevates exactly once. The daemon, CLI, and GUI never run as root - everything after the one-time setup runs as your own user.
  - icon: 🔍
    title: Self-diagnosing
    details: yerd status shows what's running; yerd doctor tells you exactly what's broken and how to fix it - and auto-repairs the safe problems.
---

<div class="home-showcase">

<h2 class="home-showcase__heading">See Yerd in action</h2>
<p class="home-showcase__sub">A tiny tray app over the whole toolchain - PHP, sites, services, mail, and Laravel telemetry, all in one place.</p>

<ShowcaseRow
  title="Multiple PHP versions"
  description="Run as many PHP versions as you need, side by side. Set a global default, pin per-site versions, tune the shared ini settings, and update in place - every version is an isolated static build the daemon supervises."
  light="/images/php-light.png"
  dark="/images/php-dark.png"
/>

<ShowcaseRow
  reverse
  title="Developer tooling, managed"
  description="Composer, Node, and Bun installed onto your PATH alongside PHP and managed by Yerd - no global installs to collide with, no version juggling. Add or remove a tool in a click."
  light="/images/tooling-light.png"
  dark="/images/tooling-dark.png"
/>

<ShowcaseRow
  title="Every project, instantly served"
  description="Park a folder and every project inside it is live on its own .test domain automatically, or link a single directory under a name you choose. Per-site PHP version and one-click HTTPS, all from one list."
  light="/images/sites-light.png"
  dark="/images/sites-dark.png"
/>

<ShowcaseRow
  reverse
  title="Databases & caches on tap"
  description="Redis, MySQL, MariaDB, and PostgreSQL supervised as native processes. Install a version, create and back up databases, and copy a ready-made Laravel .env - every installed engine starts with the daemon."
  light="/images/services-light.png"
  dark="/images/services-dark.png"
/>

<ShowcaseRow
  title="Catch every outgoing email"
  description="A built-in SMTP server captures everything your app sends during development so you can preview it - nothing ever leaves your machine. Copy the Laravel mail config and you're wired up in seconds."
  light="/images/mail-light.png"
  dark="/images/mail-dark.png"
/>

<ShowcaseRow
  reverse
  title="Laravel dumps, live"
  description="dump() and dd() plus queries, jobs, views, requests, logs, cache, and outgoing HTTP - streamed live to a dedicated window with zero code changes, captured by a native PHP extension."
  light="/images/dumps-light.png"
  dark="/images/dumps-dark.png"
/>

</div>
