import { withMermaid } from 'vitepress-plugin-mermaid'

// Site config for the Yerd documentation (https://yerd.app).
// Run with `npm run dev` from the `docs/` directory.
// Wrapped with withMermaid so ```mermaid code blocks render as responsive SVG.
export default withMermaid({
  title: 'Yerd',
  description:
    'A fast, rootless, open-source local PHP development environment. Serve .test sites over HTTP and HTTPS with a different PHP version per site.',
  lang: 'en-US',

  // Canonical host for the generated sitemap and absolute URLs.
  sitemap: {
    hostname: 'https://yerd.app',
  },

  // Clean URLs (`/guide/getting-started` instead of `.html`).
  cleanUrls: true,
  lastUpdated: true,

  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' }],
    ['meta', { name: 'theme-color', content: '#6366f1' }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:url', content: 'https://yerd.app/' }],
    ['meta', { property: 'og:title', content: 'Yerd' }],
    [
      'meta',
      {
        property: 'og:description',
        content:
          'A fast, rootless, open-source local PHP development environment.',
      },
    ],
  ],

  // Render node labels as HTML so long text wraps instead of being clipped,
  // and scale diagrams to the container width (good on small screens).
  mermaid: {
    securityLevel: 'loose',
    flowchart: { htmlLabels: true, useMaxWidth: true },
  },

  themeConfig: {
    logo: '/logo.svg',

    nav: [
      { text: 'Guide', link: '/guide/introduction', activeMatch: '/guide/' },
      { text: 'Reference', link: '/reference/cli/', activeMatch: '/reference/' },
      {
        text: 'Developer',
        link: '/developer/architecture',
        activeMatch: '/developer/',
      },
      {
        text: 'v2.0.1',
        items: [
          {
            text: 'Changelog',
            link: 'https://github.com/forjedio/yerd/releases',
          },
          { text: 'Contributing', link: '/developer/contributing' },
        ],
      },
    ],

    sidebar: {
      '/guide/': [
        {
          text: 'Introduction',
          items: [
            { text: 'What is Yerd?', link: '/guide/introduction' },
            { text: 'Getting Started', link: '/guide/getting-started' },
            { text: 'Upgrade Guide', link: '/guide/upgrading-from-v1' },
            { text: 'Features', link: '/guide/features' },
          ],
        },
        {
          text: 'Using Yerd',
          items: [
            { text: 'Sites', link: '/guide/sites' },
            { text: 'PHP Versions', link: '/guide/php-versions' },
            { text: 'Services & Databases', link: '/guide/services' },
            { text: 'HTTPS & Certificates', link: '/guide/https' },
            { text: 'DNS & .test Domains', link: '/guide/dns' },
            { text: 'Elevation & Privileges', link: '/guide/elevation' },
            { text: 'The Daemon', link: '/guide/daemon' },
            { text: 'Diagnostics', link: '/guide/diagnostics' },
            { text: 'Desktop App', link: '/guide/desktop-app' },
          ],
        },
      ],

      '/reference/': [
        {
          text: 'CLI Reference',
          items: [
            { text: 'Overview', link: '/reference/cli/' },
            { text: 'Sites', link: '/reference/cli/sites' },
            { text: 'HTTPS', link: '/reference/cli/https' },
            { text: 'PHP', link: '/reference/cli/php' },
            { text: 'Services', link: '/reference/cli/services' },
            { text: 'Databases', link: '/reference/cli/db' },
            { text: 'Diagnostics', link: '/reference/cli/diagnostics' },
            { text: 'Elevation', link: '/reference/cli/elevation' },
            { text: 'Daemon control', link: '/reference/cli/daemon' },
          ],
        },
        {
          text: 'Configuration',
          items: [
            { text: 'Config file', link: '/reference/configuration' },
          ],
        },
      ],

      '/developer/': [
        {
          text: 'Overview',
          items: [
            { text: 'Architecture', link: '/developer/architecture' },
            { text: 'Crates Overview', link: '/developer/crates' },
            { text: 'Building from Source', link: '/developer/building' },
            { text: 'Contributing', link: '/developer/contributing' },
          ],
        },
        {
          text: 'Internals',
          items: [
            { text: 'IPC Protocol', link: '/developer/ipc-protocol' },
            { text: 'Cross-Platform Model', link: '/developer/cross-platform' },
          ],
        },
        {
          text: 'Crates',
          collapsed: false,
          items: [
            { text: 'yerd-core', link: '/developer/crates/yerd-core' },
            { text: 'yerd-ipc', link: '/developer/crates/yerd-ipc' },
            { text: 'yerd-config', link: '/developer/crates/yerd-config' },
            { text: 'yerd-tls', link: '/developer/crates/yerd-tls' },
            { text: 'yerd-dns', link: '/developer/crates/yerd-dns' },
            { text: 'yerd-proxy', link: '/developer/crates/yerd-proxy' },
            { text: 'yerd-supervise', link: '/developer/crates/yerd-supervise' },
            { text: 'yerd-php', link: '/developer/crates/yerd-php' },
            { text: 'yerd-services', link: '/developer/crates/yerd-services' },
            { text: 'yerd-doctor', link: '/developer/crates/yerd-doctor' },
            { text: 'yerd-platform', link: '/developer/crates/yerd-platform' },
          ],
        },
        {
          text: 'Binaries',
          collapsed: false,
          items: [
            { text: 'yerdd (daemon)', link: '/developer/binaries/yerdd' },
            { text: 'yerd (CLI)', link: '/developer/binaries/yerd' },
            {
              text: 'yerd-helper (privileged)',
              link: '/developer/binaries/yerd-helper',
            },
          ],
        },
        {
          text: 'App & Tooling',
          items: [
            { text: 'Desktop App Internals', link: '/developer/gui' },
            { text: 'Build Automation (xtask)', link: '/developer/xtask' },
          ],
        },
      ],
    },

    socialLinks: [
      { icon: 'github', link: 'https://github.com/forjedio/yerd' },
    ],

    editLink: {
      pattern: 'https://github.com/forjedio/yerd/edit/main/docs/:path',
      text: 'Edit this page on GitHub',
    },

    search: {
      provider: 'local',
    },

    footer: {
      message:
        'A <a href="https://forjed.io" target="_blank" rel="noopener">Forjed</a> project. Released under the MIT License.',
      copyright: 'Copyright © 2026 Yerd',
    },
  },
})
