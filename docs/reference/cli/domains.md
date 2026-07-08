# Domains

Every site answers on its `{name}.test` **apex** by default. The `yerd domain`
commands let a site carry **multiple** domains, use **subdomains** and
**wildcards**, and **change** which domain is primary - without renaming the
site.

| Command | Description | Example |
| --- | --- | --- |
| `yerd domain list` | List every site's effective domains (primary marked). | `yerd domain list` |
| `yerd domain list <SITE>` | List one site's domains. | `yerd domain list blog` |
| `yerd domain add <SITE> <FQDN>` | Add an exact host or a single-label wildcard. | `yerd domain add blog corp.test` |
| `yerd domain remove <SITE> <FQDN>` | Remove a domain (a site must keep one exact domain). | `yerd domain remove blog blog.test` |
| `yerd domain primary <SITE> <FQDN>` | Set the canonical domain (added if absent; must be exact). | `yerd domain primary blog corp.test` |
| `yerd domain reset <SITE>` | Reset a site to its default apex-only domain. | `yerd domain reset blog` |

```sh
# Give one app several apex domains (multi-tenant: all served from one project)
yerd domain add app acme.test
yerd domain add app globex.test

# A single-label wildcard: every foo.test subdomain (one level) routes to blog
yerd domain add blog '*.blog.test'

# Point a specific subdomain at a *different* site (exact beats wildcard)
yerd link api ~/code/api
yerd domain add api api.blog.test

# Move a site to a new primary domain and drop the old apex
yerd domain add blog corp.test
yerd domain primary blog corp.test
yerd domain remove blog blog.test
```

`domain list` marks each site's primary domain, and appends
`[apex shadowed by <site>]` to a site whose apex label is claimed by another
site - the same condition doctor reports as its `DomainShadowed` check. With
`--json`, each site is `{name, primary, domains, apex_shadowed_by}`.

## How resolution works

For an incoming host, yerd tries an **exact** domain match first, then a
**single-label wildcard** (the host with its leftmost label replaced by `*`).
Exact always wins.

- `*.blog.test` matches exactly one label - `api.blog.test`, not
  `x.api.blog.test`. To route a deeper level, register that wildcard too:
  `yerd domain add deep '*.api.blog.test'`.
- `blog.test` and `*.blog.test` can belong to **different** sites.
- A domain can only be claimed by one site; adding a domain another site already
  holds fails with an "already routes to" error.

::: warning Subdomains are now explicit
Earlier versions routed **every** subdomain of a site (`anything.blog.test`) to
that site automatically. That implicit catch-all has been **removed**: a site
answers only its apex until you add domains. If you relied on subdomains
(e.g. WordPress multisite in subdomain mode), re-enable them explicitly:

```sh
yerd domain add blog '*.blog.test'
```
:::

::: details Client-side validation
`domain add/remove/primary` validate the site name and the domain's shape
client-side (ASCII `[a-z0-9.*-]`, `*` only as the leftmost label) before
connecting - a malformed domain fails with a usage error and exit code `2`. The
daemon validates that the domain sits under the configured TLD.
:::
