//! Attributes captured mail to a site through the live router.
//!
//! The mail sink only sees the host an app announced in SMTP `EHLO`/`HELO`.
//! [`RouterSiteResolver`] maps that host to a PHP site with the same lookup the
//! proxy uses for a `Host:` header, so ports, trailing dots, case, extra
//! domains and wildcards all behave as they do for HTTP.

use std::future::Future;

use yerd_proxy::SharedRouter;

/// A [`yerd_mail::SiteResolver`] backed by the daemon's shared site router.
pub struct RouterSiteResolver {
    /// The router swapped in on every rebuild, read per message.
    pub router: SharedRouter,
}

impl yerd_mail::SiteResolver for RouterSiteResolver {
    fn site_for_host(&self, host: &str) -> impl Future<Output = Option<String>> + Send {
        let router = self.router.clone();
        let host = host.to_owned();
        async move {
            router
                .read()
                .await
                .resolve(&host)
                .map(|s| s.name().to_owned())
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use std::sync::Arc;

    use yerd_core::{Domain, PhpVersion, RouterConfig, Site, SiteRouter};
    use yerd_mail::SiteResolver;

    use super::*;

    fn resolver() -> RouterSiteResolver {
        let mut router = SiteRouter::new(RouterConfig::new("test").unwrap());
        let site = Site::parked("blog", "/srv/blog", PhpVersion::new(8, 3)).unwrap();
        let apex = Domain::apex("blog");
        let extra = Domain::parse_subpart("news").unwrap();
        router
            .insert_with_domains(site, vec![apex.clone(), extra], apex)
            .unwrap();
        RouterSiteResolver {
            router: Arc::new(tokio::sync::RwLock::new(router)),
        }
    }

    #[tokio::test]
    async fn resolves_ehlo_hosts_to_sites() {
        let r = resolver();
        let cases: &[(&str, Option<&str>)] = &[
            ("blog.test", Some("blog")),
            ("BLOG.TEST.", Some("blog")),
            ("news.test", Some("blog")),
            ("[127.0.0.1]", None),
            ("example.com", None),
            ("localhost", None),
        ];
        for (host, want) in cases {
            assert_eq!(r.site_for_host(host).await.as_deref(), *want, "host {host}");
        }
    }
}
