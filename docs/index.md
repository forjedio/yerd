---
layout: home

hero:
  name: Yerd
  text: Local PHP, without the friction.
  tagline: Serve your projects on .test domains over HTTP and HTTPS, run a different PHP version per site, and manage it all from one tiny daemon. No Docker, no sudo for everyday work, no subscription.
  image:
    src: /logo.svg
    alt: Yerd
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
