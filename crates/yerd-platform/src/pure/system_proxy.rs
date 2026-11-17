//! Pure parsers for OS proxy settings: GNOME `gsettings list-recursively
//! org.gnome.system.proxy` output and the macOS `SCDynamicStore` proxy
//! dictionary as printed by `scutil --proxy` ([`parse_scutil_proxy`] into
//! [`MacProxySettings`]).
//!
//! Neither source's credentials are read: GNOME keeps them unencoded and
//! macOS keeps them in the keychain. Users who need an authenticated proxy set
//! it manually.

use yerd_core::network::{normalize_bypass_entry, ProxyUrl, SystemProxy};

fn proxy_url(host: &str, port: Option<i64>) -> Option<ProxyUrl> {
    let host = host.trim();
    if host.is_empty() {
        return None;
    }
    let port = port.filter(|p| *p > 0);
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let raw = match port {
        Some(p) => format!("http://{host}:{p}"),
        None => format!("http://{host}"),
    };
    ProxyUrl::parse(&raw).ok()
}

fn gvariant_string(value: &str) -> String {
    let v = value.trim();
    v.strip_prefix('\'')
        .and_then(|s| s.strip_suffix('\''))
        .unwrap_or(v)
        .to_owned()
}

fn gvariant_string_list(value: &str) -> Vec<String> {
    let v = value.trim();
    let v = v.strip_prefix("@as ").unwrap_or(v);
    let inner = v
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or("");
    inner
        .split(',')
        .map(gvariant_string)
        .filter(|s| !s.is_empty())
        .collect()
}

/// Parses `gsettings list-recursively org.gnome.system.proxy` output.
///
/// Mode `manual` yields the HTTP/HTTPS host and port; `auto` flags PAC; `none`
/// yields an empty reading. `None` when the output has no `mode` line (schema
/// missing or garbage).
#[must_use]
pub fn parse_gsettings(output: &str) -> Option<SystemProxy> {
    let mut mode: Option<String> = None;
    let mut plain_host = String::new();
    let mut plain_port: Option<i64> = None;
    let mut secure_host = String::new();
    let mut secure_port: Option<i64> = None;
    let mut ignore: Vec<String> = Vec::new();
    for line in output.lines() {
        let mut parts = line.trim().splitn(3, ' ');
        let (Some(schema), Some(key), Some(value)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        match (schema, key) {
            ("org.gnome.system.proxy", "mode") => mode = Some(gvariant_string(value)),
            ("org.gnome.system.proxy", "ignore-hosts") => ignore = gvariant_string_list(value),
            ("org.gnome.system.proxy.http", "host") => plain_host = gvariant_string(value),
            ("org.gnome.system.proxy.http", "port") => plain_port = value.trim().parse().ok(),
            ("org.gnome.system.proxy.https", "host") => secure_host = gvariant_string(value),
            ("org.gnome.system.proxy.https", "port") => secure_port = value.trim().parse().ok(),
            _ => {}
        }
    }
    let mode = mode?;
    let bypass = || {
        ignore
            .iter()
            .filter_map(|e| normalize_bypass_entry(e))
            .collect()
    };
    match mode.as_str() {
        "manual" => Some(SystemProxy {
            http: proxy_url(&plain_host, plain_port),
            https: proxy_url(&secure_host, secure_port),
            bypass: bypass(),
            pac_detected: false,
        }),
        "auto" => Some(SystemProxy {
            pac_detected: true,
            bypass: bypass(),
            ..SystemProxy::default()
        }),
        _ => Some(SystemProxy::default()),
    }
}

/// The fields of the macOS proxy dictionary Yerd reads, already extracted
/// from Core Foundation types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct MacProxySettings {
    /// `HTTPEnable`.
    pub http_enable: bool,
    /// `HTTPProxy`.
    pub http_host: Option<String>,
    /// `HTTPPort`.
    pub http_port: Option<i64>,
    /// `HTTPSEnable`.
    pub https_enable: bool,
    /// `HTTPSProxy`.
    pub https_host: Option<String>,
    /// `HTTPSPort`.
    pub https_port: Option<i64>,
    /// `ExceptionsList`.
    pub exceptions: Vec<String>,
    /// `ProxyAutoConfigEnable`.
    pub pac_enable: bool,
    /// `ProxyAutoDiscoveryEnable`.
    pub auto_discovery_enable: bool,
}

impl MacProxySettings {
    /// Converts to a [`SystemProxy`]. Both schemes use an `http://` proxy URL:
    /// macOS's "Secure Web Proxy" is still spoken to over plain HTTP CONNECT.
    /// `ExcludeSimpleHostnames` is not honoured.
    #[must_use]
    pub fn into_system_proxy(self) -> SystemProxy {
        let pick = |enabled: bool, host: Option<&String>, port: Option<i64>| {
            if enabled {
                host.and_then(|h| proxy_url(h, port))
            } else {
                None
            }
        };
        SystemProxy {
            http: pick(self.http_enable, self.http_host.as_ref(), self.http_port),
            https: pick(self.https_enable, self.https_host.as_ref(), self.https_port),
            bypass: self
                .exceptions
                .iter()
                .filter_map(|e| normalize_bypass_entry(e))
                .collect(),
            pac_detected: self.pac_enable || self.auto_discovery_enable,
        }
    }
}

/// Parses `scutil --proxy` output (the `SCDynamicStore` proxy dictionary).
///
/// ```text
/// <dictionary> {
///   ExceptionsList : <array> {
///     0 : *.local
///   }
///   HTTPEnable : 1
///   HTTPPort : 8080
///   HTTPProxy : proxy.corp
/// }
/// ```
///
/// `None` when the output is not a dictionary.
#[must_use]
pub fn parse_scutil_proxy(output: &str) -> Option<MacProxySettings> {
    if !output.trim_start().starts_with("<dictionary>") {
        return None;
    }
    let mut s = MacProxySettings::default();
    let mut in_exceptions = false;
    for line in output.lines() {
        let line = line.trim();
        if in_exceptions {
            if line == "}" {
                in_exceptions = false;
            } else if let Some((_, value)) = line.split_once(" : ") {
                s.exceptions.push(value.trim().to_owned());
            }
            continue;
        }
        let Some((key, value)) = line.split_once(" : ") else {
            continue;
        };
        let value = value.trim();
        let flag = value == "1";
        let number = value.parse::<i64>().ok();
        match key.trim() {
            "ExceptionsList" => in_exceptions = value.starts_with("<array>"),
            "HTTPEnable" => s.http_enable = flag,
            "HTTPProxy" => s.http_host = Some(value.to_owned()),
            "HTTPPort" => s.http_port = number,
            "HTTPSEnable" => s.https_enable = flag,
            "HTTPSProxy" => s.https_host = Some(value.to_owned()),
            "HTTPSPort" => s.https_port = number,
            "ProxyAutoConfigEnable" => s.pac_enable = flag,
            "ProxyAutoDiscoveryEnable" => s.auto_discovery_enable = flag,
            _ => {}
        }
    }
    Some(s)
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn gnome(mode: &str, host: &str, port: u16) -> String {
        format!(
            "org.gnome.system.proxy autoconfig-url ''\n\
             org.gnome.system.proxy ignore-hosts ['localhost', '127.0.0.0/8', '::1', '*.corp.com']\n\
             org.gnome.system.proxy mode '{mode}'\n\
             org.gnome.system.proxy use-same-proxy true\n\
             org.gnome.system.proxy.http authentication-password ''\n\
             org.gnome.system.proxy.http host '{host}'\n\
             org.gnome.system.proxy.http port {port}\n\
             org.gnome.system.proxy.https host '{host}'\n\
             org.gnome.system.proxy.https port {port}\n\
             org.gnome.system.proxy.socks host ''\n"
        )
    }

    #[test]
    fn gsettings_table() {
        let none = parse_gsettings(&gnome("none", "", 0)).unwrap();
        assert!(none.is_empty());
        assert!(!none.pac_detected);

        let manual = parse_gsettings(&gnome("manual", "proxy.corp", 3128)).unwrap();
        assert_eq!(
            manual.http.as_ref().map(ProxyUrl::as_str),
            Some("http://proxy.corp:3128")
        );
        assert_eq!(
            manual.https.as_ref().map(ProxyUrl::as_str),
            Some("http://proxy.corp:3128")
        );
        assert_eq!(
            manual.bypass,
            vec!["localhost", "127.0.0.0/8", "::1", ".corp.com"]
        );

        let auto = parse_gsettings(&gnome("auto", "", 0)).unwrap();
        assert!(auto.is_empty());
        assert!(auto.pac_detected);

        let manual_empty = parse_gsettings(&gnome("manual", "", 0)).unwrap();
        assert!(manual_empty.is_empty());

        assert!(parse_gsettings("No such schema “org.gnome.system.proxy”\n").is_none());
        assert!(parse_gsettings("").is_none());
    }

    #[test]
    fn gsettings_empty_ignore_list() {
        let text =
            "org.gnome.system.proxy ignore-hosts @as []\norg.gnome.system.proxy mode 'none'\n";
        assert!(parse_gsettings(text).unwrap().bypass.is_empty());
    }

    #[test]
    fn mac_table() {
        let http_only = MacProxySettings {
            http_enable: true,
            http_host: Some("proxy.corp".into()),
            http_port: Some(8080),
            ..MacProxySettings::default()
        }
        .into_system_proxy();
        assert_eq!(
            http_only.http.as_ref().map(ProxyUrl::as_str),
            Some("http://proxy.corp:8080")
        );
        assert!(http_only.https.is_none());

        let both = MacProxySettings {
            http_enable: true,
            http_host: Some("h".into()),
            http_port: Some(1),
            https_enable: true,
            https_host: Some("s".into()),
            https_port: Some(2),
            exceptions: vec!["*.local".into(), "169.254/16".into()],
            ..MacProxySettings::default()
        }
        .into_system_proxy();
        assert_eq!(
            both.https.as_ref().map(ProxyUrl::as_str),
            Some("http://s:2")
        );
        assert_eq!(both.bypass, vec![".local", "169.254.0.0/16"]);

        let pac = MacProxySettings {
            pac_enable: true,
            ..MacProxySettings::default()
        }
        .into_system_proxy();
        assert!(pac.pac_detected);
        assert!(pac.is_empty());

        let wpad = MacProxySettings {
            auto_discovery_enable: true,
            ..MacProxySettings::default()
        }
        .into_system_proxy();
        assert!(wpad.pac_detected);

        let disabled = MacProxySettings {
            http_enable: false,
            http_host: Some("h".into()),
            http_port: Some(1),
            ..MacProxySettings::default()
        }
        .into_system_proxy();
        assert!(disabled.is_empty());
    }

    #[test]
    fn scutil_table() {
        let full = "<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n    1 : 169.254/16\n  }\n  FTPPassive : 1\n  HTTPEnable : 1\n  HTTPPort : 8080\n  HTTPProxy : proxy.corp\n  HTTPSEnable : 1\n  HTTPSPort : 8443\n  HTTPSProxy : secure.corp\n  ProxyAutoConfigEnable : 0\n}\n";
        assert_eq!(
            parse_scutil_proxy(full).unwrap(),
            MacProxySettings {
                http_enable: true,
                http_host: Some("proxy.corp".into()),
                http_port: Some(8080),
                https_enable: true,
                https_host: Some("secure.corp".into()),
                https_port: Some(8443),
                exceptions: vec!["*.local".into(), "169.254/16".into()],
                pac_enable: false,
                auto_discovery_enable: false,
            }
        );

        let pac = "<dictionary> {\n  ProxyAutoConfigEnable : 1\n  ProxyAutoConfigURLString : http://wpad/proxy.pac\n}\n";
        let r = parse_scutil_proxy(pac).unwrap().into_system_proxy();
        assert!(r.pac_detected);
        assert!(r.is_empty());

        let none = "<dictionary> {\n  ExceptionsList : <array> {\n    0 : *.local\n  }\n  FTPPassive : 1\n}\n";
        let r = parse_scutil_proxy(none).unwrap().into_system_proxy();
        assert!(r.is_empty());
        assert_eq!(r.bypass, vec![".local"]);

        assert!(parse_scutil_proxy("").is_none());
        assert!(parse_scutil_proxy("No proxy").is_none());
    }

    #[test]
    fn ipv6_host_is_bracketed() {
        let r = MacProxySettings {
            http_enable: true,
            http_host: Some("::1".into()),
            http_port: Some(3128),
            ..MacProxySettings::default()
        }
        .into_system_proxy();
        assert_eq!(
            r.http.as_ref().map(ProxyUrl::as_str),
            Some("http://[::1]:3128")
        );
    }
}
