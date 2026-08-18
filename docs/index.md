---
layout: HomeLayout
pageClass: home-page

# A descriptive <title> for the homepage (the default would just be "Yerd").
title: Yerd - Local PHP development environment for macOS & Linux
titleTemplate: false
description: Serve every project in a folder on its own .test domain over HTTPS, run a different PHP version per site, and manage databases, mail and tooling from one ~8 MB Rust daemon. Free, MIT-licensed, rootless, and Docker-free.

hero:
  pill: OPEN SOURCE · ROOTLESS · MIT
  lines:
    - LOCAL PHP,
    - WITHOUT THE
  accent: WEIGHT.
  tagline: Every project in a parked folder is live on its own .test domain - HTTPS, per-site PHP, databases and mail. One tiny Rust daemon, and no Docker anywhere.
  command:
    label: YOUR FIRST COMMAND
    text: yerd park ~/Code
  actions:
    - theme: brand
      text: Get started →
      link: /guide/getting-started
    - theme: alt
      text: View on GitHub
      link: https://github.com/forjedio/yerd
  platforms: macOS (Apple Silicon) · Debian · Ubuntu · Arch · Fedora
  shot:
    light: /images/overview-light.png
    dark: /images/overview-dark.png
    alt: The Yerd desktop app Overview dashboard

stats:
  - value: 8 MB
    label: ONE RUST DAEMON
  - value: '0'
    label: CONTAINERS OR VMS
  - value: 1 CMD
    label: FOLDER TO LIVE SITES
  - value: MIT
    label: FREE AND OPEN SOURCE

footprint:
  eyebrow: FOOTPRINT
  heading: THE BLUE SLIVER IS YERD.
  sub: The bar below stands for 1.6 GB - about what a container runtime costs you before a single site is served. Yerd's daemon is the few pixels glowing on the left.
  scaleMax: 1600
  ticks: ['0', 400 MB, 800 MB, 1.2 GB, 1.6 GB]
  yerd:
    label: YERD · ~8 MB
    mb: 8
  middle:
    label: ELECTRON APP · ~180 MB
    mb: 180
  full:
    label: CONTAINER RUNTIME · ~1.6 GB

capabilities:
  eyebrow: WHAT IT DOES
  heading: EVERY CAPABILITY HAS A COMMAND.
  sub: The desktop app and the CLI drive the same daemon. Whatever you can click, you can script.
  items:
    - icon: grid
      title: Zero-config sites
      details: Drop a project into a parked folder and it is live at its own .test domain straight away.
      command: yerd park ~/Code
      link: /guide/sites
    - icon: lock
      title: Automatic HTTPS
      details: A local certificate authority issues a per-site certificate on demand. No mkcert, no browser warnings.
      command: yerd secure my-app
      link: /guide/https
    - icon: stack
      title: Per-site PHP
      details: Install as many PHP versions as you need, set a global default, then pin the sites that disagree.
      command: yerd use 8.4
      link: /guide/php-versions
    - icon: database
      title: Databases & caches
      details: MySQL, MariaDB, PostgreSQL, Redis and Meilisearch, supervised as native processes and started with the daemon.
      command: yerd db create app
      link: /guide/services
    - icon: mail
      title: Mail, caught locally
      details: Every message your apps send is captured on the machine and readable in the app. Nothing escapes.
      command: yerd mail list
      link: /guide/mail
    - icon: shield
      title: Self-diagnosing
      details: Status shows what is running. Doctor tells you exactly what is broken, and repairs the safe problems.
      command: yerd doctor fix
      link: /guide/diagnostics

steps:
  eyebrow: FROM ZERO
  heading: THREE COMMANDS TO A WORKING SITE.
  items:
    - command: yerd install php 8.5
      details: PHP is not bundled. Yerd downloads a prebuilt static build on demand and supervises its FPM pool for you.
    - command: yerd park ~/Code
      details: Every project inside the folder is served on its own .test domain, immediately and with no config file.
    - command: yerd secure my-app
      details: A certificate issued on demand by your own local CA. Green padlock, no browser warnings, no mkcert.

app:
  eyebrow: THE APP
  heading: ONE WINDOW OVER THE WHOLE TOOLCHAIN.
  sub: Sites, PHP versions, services, mail and Laravel telemetry - the desktop app is a thin window over the same daemon the CLI drives.
  primary:
    light: /images/overview-light.png
    dark: /images/overview-dark.png
    alt: The Yerd desktop app Overview dashboard
  secondary:
    - light: /images/sites-light.png
      dark: /images/sites-dark.png
      alt: The Sites page in the Yerd desktop app
    - light: /images/services-light.png
      dark: /images/services-dark.png
      alt: The Services page in the Yerd desktop app
  captions:
    - OVERVIEW · every project and service at a glance
    - SITES · SERVICES · managed in place
  chipsLabel: ALSO INSTALLED ON DEMAND
  chips:
    - MySQL
    - MariaDB
    - PostgreSQL
    - Redis
    - Meilisearch
    - SMTP mail
    - Composer
    - Node
    - Bun
    - Laravel dumps
    - LAN sharing

closing:
  lines:
    - STOP RUNNING A DATA CENTRE
    - TO BUILD A WEBSITE.
  sub: Free and MIT-licensed. macOS and Linux. No account, no subscription.
  actions:
    - theme: brand
      text: Get started →
      link: /guide/getting-started
    - theme: alt
      text: View on GitHub
      link: https://github.com/forjedio/yerd
---
