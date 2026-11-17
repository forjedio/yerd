//! Pure outbound-proxy model: how Yerd's own outbound HTTP traffic (downloads,
//! self-update, site creation tools) reaches the internet.
//!
//! Three modes ([`OutboundProxyMode`]): `system` auto-detects from the daemon
//! environment, the OS settings, then the user's login shell; `manual` uses
//! user-entered URLs; `off` connects directly. Every decision lives here:
//! URL validation ([`ProxyUrl`]), bypass normalisation ([`bypass_list`]),
//! source precedence ([`resolve`]), credential redaction ([`scrub`],
//! [`merge_submitted`]) and the child-process environment
//! ([`child_env_plan`], [`render_env_file`], [`shim_fill_in`]).
//!
//! The module does no I/O: the daemon gathers the env, OS, and login-shell
//! readings and hands them in.

use std::fmt;
use std::fmt::Write as _;
use std::net::IpAddr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::error::{CoreError, ProxyUrlErrorReason};
use crate::proxy::{is_valid_host, split_host_port};

/// How the daemon picks its outbound proxy.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutboundProxyMode {
    /// Detect from the daemon env, OS settings, then the login shell.
    #[default]
    System,
    /// Use the user-entered URLs.
    Manual,
    /// Connect directly, ignoring any proxy environment.
    Off,
}

impl OutboundProxyMode {
    /// The wire and config spelling (`system`, `manual`, `off`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Manual => "manual",
            Self::Off => "off",
        }
    }
}

impl FromStr for OutboundProxyMode {
    type Err = CoreError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "system" => Ok(Self::System),
            "manual" => Ok(Self::Manual),
            "off" => Ok(Self::Off),
            _ => Err(CoreError::InvalidProxyMode {
                input: s.to_owned(),
            }),
        }
    }
}

/// Where the resolved proxy came from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProxySource {
    /// The user-entered URLs (manual mode).
    Manual,
    /// The daemon's own `*_proxy` environment variables.
    DaemonEnv,
    /// macOS System Settings (`SCDynamicStore`).
    MacosSystemSettings,
    /// GNOME `org.gnome.system.proxy` settings.
    GnomeSettings,
    /// `*_proxy` exports in the user's login shell.
    LoginShell,
    /// No proxy was resolved.
    #[default]
    None,
}

impl ProxySource {
    /// A short human label for status lines and Doctor messages.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Manual => "manual setting",
            Self::DaemonEnv => "daemon environment",
            Self::MacosSystemSettings => "macOS System Settings",
            Self::GnomeSettings => "GNOME settings",
            Self::LoginShell => "login shell",
            Self::None => "none",
        }
    }
}

/// A validated `http[s]://[user[:password]@]host:port` proxy URL.
///
/// The scheme defaults to `http` and the port to 80/443, but the canonical
/// form ([`Self::as_str`]) always spells both. Userinfo is kept verbatim
/// (percent-encoding included). `Debug` prints [`Self::redacted`], so a
/// derived `Debug` on a containing struct never leaks the password.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ProxyUrl {
    secure: bool,
    user: Option<String>,
    password: Option<String>,
    host: String,
    port: u16,
    canonical: String,
}

impl ProxyUrl {
    /// Parses a proxy URL.
    ///
    /// # Errors
    /// [`CoreError::InvalidProxyUrl`] naming the failed rule; the error never
    /// carries the input.
    pub fn parse(input: &str) -> Result<Self, CoreError> {
        let err = |reason| CoreError::InvalidProxyUrl { reason };
        let raw = input.trim();
        if raw.is_empty() {
            return Err(err(ProxyUrlErrorReason::Empty));
        }
        let (secure, rest) = match raw.split_once("://") {
            Some((scheme, rest)) => match scheme.to_ascii_lowercase().as_str() {
                "http" => (false, rest),
                "https" => (true, rest),
                _ => return Err(err(ProxyUrlErrorReason::UnsupportedScheme)),
            },
            None => (false, raw),
        };
        let rest = rest.strip_suffix('/').unwrap_or(rest);
        if rest.contains(['/', '?', '#']) {
            return Err(err(ProxyUrlErrorReason::HasPathOrQuery));
        }
        let (userinfo, hostport) = match rest.rsplit_once('@') {
            Some((u, h)) => (Some(u), h),
            None => (None, rest),
        };
        let (user, password) = match userinfo {
            Some(u) => {
                let (user, password) =
                    parse_userinfo(u).ok_or_else(|| err(ProxyUrlErrorReason::InvalidUserinfo))?;
                (Some(user), password)
            }
            None => (None, None),
        };
        let default_port = if secure { 443 } else { 80 };
        let (host, port) = split_host_port(hostport, default_port)
            .ok_or_else(|| err(ProxyUrlErrorReason::InvalidPort))?;
        if host.is_empty() {
            return Err(err(ProxyUrlErrorReason::MissingHost));
        }
        if !is_valid_host(host) {
            return Err(err(ProxyUrlErrorReason::InvalidHost));
        }
        if port == 0 {
            return Err(err(ProxyUrlErrorReason::InvalidPort));
        }
        let host = host.to_ascii_lowercase();
        let canonical = render(secure, user.as_deref(), password.as_deref(), &host, port);
        Ok(Self {
            secure,
            user,
            password,
            host,
            port,
            canonical,
        })
    }

    /// The canonical URL, credentials included. Only for building a client or
    /// a child environment; use [`Self::redacted`] for anything shown.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.canonical
    }

    /// The canonical URL with any password replaced by `***`.
    #[must_use]
    pub fn redacted(&self) -> String {
        let password = self.password.as_ref().map(|_| REDACTED);
        render(
            self.secure,
            self.user.as_deref(),
            password,
            &self.host,
            self.port,
        )
    }

    /// Whether the scheme is `https`.
    #[must_use]
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// The host, unbracketed even for IPv6.
    #[must_use]
    pub fn host(&self) -> &str {
        &self.host
    }

    /// The port (always explicit after parsing).
    #[must_use]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// The userinfo user, if any.
    #[must_use]
    pub fn user(&self) -> Option<&str> {
        self.user.as_deref()
    }

    /// Whether the URL carries a password.
    #[must_use]
    pub fn has_password(&self) -> bool {
        self.password.is_some()
    }

    fn same_endpoint(&self, other: &Self) -> bool {
        self.secure == other.secure
            && self.user == other.user
            && self.host == other.host
            && self.port == other.port
    }
}

impl fmt::Debug for ProxyUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("ProxyUrl").field(&self.redacted()).finish()
    }
}

/// The placeholder that stands in for a password in every displayed URL.
pub const REDACTED: &str = "***";

fn parse_userinfo(raw: &str) -> Option<(String, Option<String>)> {
    if raw.is_empty() || raw.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let (user, password) = match raw.split_once(':') {
        Some((u, p)) => (u, Some(p.to_owned())),
        None => (raw, None),
    };
    if user.is_empty() {
        return None;
    }
    Some((user.to_owned(), password))
}

fn render(
    secure: bool,
    user: Option<&str>,
    password: Option<&str>,
    host: &str,
    port: u16,
) -> String {
    let scheme = if secure { "https" } else { "http" };
    let auth = match (user, password) {
        (Some(u), Some(p)) => format!("{u}:{p}@"),
        (Some(u), None) => format!("{u}@"),
        _ => String::new(),
    };
    if host.contains(':') {
        format!("{scheme}://{auth}[{host}]:{port}")
    } else {
        format!("{scheme}://{auth}{host}:{port}")
    }
}

/// One proxy reading from the environment, the OS, or the login shell.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemProxy {
    /// Proxy for `http://` requests.
    pub http: Option<ProxyUrl>,
    /// Proxy for `https://` requests.
    pub https: Option<ProxyUrl>,
    /// Bypass entries, already normalised.
    pub bypass: Vec<String>,
    /// Whether a PAC script or WPAD auto-discovery is configured. Yerd cannot
    /// evaluate PAC; Doctor warns instead.
    pub pac_detected: bool,
}

impl SystemProxy {
    /// True when the reading names no proxy for either scheme.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.http.is_none() && self.https.is_none()
    }
}

/// The proxy-related environment variable names, both cases.
///
/// The login-shell capture prints exactly these, and Off mode removes them all
/// from a child's environment.
pub const PROXY_ENV_NAMES: [&str; 8] = [
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
    "no_proxy",
    "NO_PROXY",
];

/// File name, under the data directory, of the rendered proxy environment the
/// daemon writes and the `{data}/bin` shims read.
pub const ENV_FILE_NAME: &str = "network.env";

/// The keys a shim accepts from the env file: [`PROXY_ENV_NAMES`] plus
/// [`NODE_USE_ENV_PROXY`].
pub const SHIM_ENV_NAMES: [&str; 9] = [
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
    "no_proxy",
    "NO_PROXY",
    NODE_USE_ENV_PROXY,
];

/// Makes Node 24+ honour `HTTP(S)_PROXY`. Written only to the env file and
/// child env, never read from the login shell.
pub const NODE_USE_ENV_PROXY: &str = "NODE_USE_ENV_PROXY";

fn first_proxy(lookup: &dyn Fn(&str) -> Option<String>, names: &[&str]) -> Option<ProxyUrl> {
    names
        .iter()
        .filter_map(|n| lookup(n))
        .filter(|v| !v.trim().is_empty())
        .find_map(|v| ProxyUrl::parse(&v).ok())
}

/// Reads a [`SystemProxy`] from `*_proxy` style variables.
///
/// Lowercase names win over uppercase. `all_proxy` fills only the schemes
/// still unset. Invalid values and SOCKS URLs are skipped.
#[must_use]
pub fn from_env_lookup(lookup: &dyn Fn(&str) -> Option<String>) -> SystemProxy {
    let all = first_proxy(lookup, &["all_proxy", "ALL_PROXY"]);
    let http = first_proxy(lookup, &["http_proxy", "HTTP_PROXY"]).or_else(|| all.clone());
    let https = first_proxy(lookup, &["https_proxy", "HTTPS_PROXY"]).or(all);
    let bypass = ["no_proxy", "NO_PROXY"]
        .iter()
        .filter_map(|n| lookup(n))
        .find(|v| !v.trim().is_empty())
        .map(|v| {
            v.split(|c: char| c == ',' || c.is_whitespace())
                .filter_map(normalize_bypass_entry)
                .collect()
        })
        .unwrap_or_default();
    SystemProxy {
        http,
        https,
        bypass,
        pac_detected: false,
    }
}

/// Entries that are always bypassed, whatever the mode or source.
pub const MANDATORY_BYPASS: [&str; 4] = ["localhost", "127.0.0.1", "127.0.0.0/8", "::1"];

/// Normalises one bypass entry into a form reqwest, curl, and Node all
/// understand.
///
/// A plain host such as `corp.com` stays as it is (all three already match it
/// and its subdomains); a `*.corp.com` wildcard becomes `.corp.com`; a leading
/// dot is kept; a short IPv4 CIDR such as `169.254/16` is padded; `host:port`
/// drops the port. Empty entries, `<local>`, and any other `*` pattern yield
/// `None`.
#[must_use]
pub fn normalize_bypass_entry(raw: &str) -> Option<String> {
    let entry = raw.trim().to_ascii_lowercase();
    if entry.is_empty() || entry == "<local>" {
        return None;
    }
    if entry == "*" {
        return Some(entry);
    }
    if let Some((addr, bits)) = entry.split_once('/') {
        return normalize_cidr(addr, bits);
    }
    let unbracketed = entry
        .strip_prefix('[')
        .and_then(|e| e.split_once(']'))
        .map(|(inner, _)| inner.to_owned());
    if let Some(inner) = unbracketed {
        return inner.parse::<IpAddr>().ok().map(|ip| ip.to_string());
    }
    if entry.parse::<IpAddr>().is_ok() {
        return Some(entry);
    }
    let host = match entry.rsplit_once(':') {
        Some((h, port)) if port.bytes().all(|b| b.is_ascii_digit()) => h,
        Some(_) => return None,
        None => entry.as_str(),
    };
    if host.parse::<IpAddr>().is_ok() {
        return Some(host.to_owned());
    }
    let (dotted, domain) = match host.strip_prefix("*.") {
        Some(rest) => (true, rest),
        None => match host.strip_prefix('.') {
            Some(rest) => (true, rest),
            None => (false, host),
        },
    };
    let valid_label = |l: &str| {
        !l.is_empty()
            && l.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    };
    if !domain.split('.').all(valid_label) {
        return None;
    }
    if dotted {
        Some(format!(".{domain}"))
    } else {
        Some(domain.to_owned())
    }
}

/// Normalises every entry with [`normalize_bypass_entry`], dropping the
/// rejected ones and duplicates while keeping first-seen order.
#[must_use]
pub fn normalize_bypass_entries(entries: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for entry in entries.iter().filter_map(|e| normalize_bypass_entry(e)) {
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

fn normalize_cidr(addr: &str, bits: &str) -> Option<String> {
    let prefix: u8 = bits.parse().ok()?;
    if let Ok(ip) = addr.parse::<IpAddr>() {
        let max = if ip.is_ipv4() { 32 } else { 128 };
        return (prefix <= max).then(|| format!("{ip}/{prefix}"));
    }
    let octets: Vec<&str> = addr.split('.').collect();
    if octets.is_empty()
        || octets.len() > 4
        || prefix > 32
        || !octets.iter().all(|o| o.parse::<u8>().is_ok())
    {
        return None;
    }
    let mut padded: Vec<&str> = octets;
    padded.resize(4, "0");
    Some(format!("{}/{prefix}", padded.join(".")))
}

/// Builds the effective bypass list: [`MANDATORY_BYPASS`], `.<tld>`, then the
/// user entries, then the source's entries, normalised and de-duplicated in
/// first-seen order.
#[must_use]
pub fn bypass_list(tld: &str, user: &[String], system: &[String]) -> Vec<String> {
    let tld_entry = format!(".{}", tld.trim_start_matches('.'));
    let all: Vec<String> = MANDATORY_BYPASS
        .iter()
        .map(|s| (*s).to_owned())
        .chain(std::iter::once(tld_entry))
        .chain(user.iter().cloned())
        .chain(system.iter().cloned())
        .collect();
    normalize_bypass_entries(&all)
}

/// Whether a request to `host` skips the proxy under `bypass`, matching the
/// rules reqwest's `NoProxy` applies (the daemon's client uses that matcher
/// with the same list).
///
/// `*` bypasses everything; an IP or CIDR entry matches IP hosts; `.corp.com`
/// and `corp.com` both match `corp.com` and every subdomain. Comparison ignores
/// case, a trailing dot, and IPv6 brackets.
#[must_use]
pub fn bypasses(host: &str, bypass: &[String]) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&host);
    let ip = host.parse::<IpAddr>().ok();
    bypass.iter().any(|entry| {
        let entry = entry.trim().to_ascii_lowercase();
        if entry == "*" {
            return true;
        }
        if let Some(ip) = ip {
            return ip_matches(ip, &entry);
        }
        let domain = entry.strip_prefix('.').unwrap_or(&entry);
        !domain.is_empty()
            && (host == domain
                || host
                    .strip_suffix(domain)
                    .is_some_and(|prefix| prefix.ends_with('.')))
    })
}

fn ip_matches(ip: IpAddr, entry: &str) -> bool {
    let Some((addr, bits)) = entry.split_once('/') else {
        return entry.parse::<IpAddr>().is_ok_and(|e| e == ip);
    };
    let (Ok(net), Ok(bits)) = (addr.parse::<IpAddr>(), bits.parse::<u32>()) else {
        return false;
    };
    match (ip, net) {
        (IpAddr::V4(ip), IpAddr::V4(net)) if bits <= 32 => {
            let mask = u32::MAX.checked_shl(32 - bits).unwrap_or(0);
            u32::from(ip) & mask == u32::from(net) & mask
        }
        (IpAddr::V6(ip), IpAddr::V6(net)) if bits <= 128 => {
            let mask = u128::MAX.checked_shl(128 - bits).unwrap_or(0);
            u128::from(ip) & mask == u128::from(net) & mask
        }
        _ => false,
    }
}

/// Everything [`resolve`] decides from.
#[derive(Debug, Clone, Copy)]
pub struct ResolveInput<'a> {
    /// Configured mode.
    pub mode: OutboundProxyMode,
    /// Manual HTTP proxy.
    pub manual_http: Option<&'a ProxyUrl>,
    /// Manual HTTPS proxy; falls back to [`Self::manual_http`] when `None`.
    pub manual_https: Option<&'a ProxyUrl>,
    /// User bypass entries.
    pub user_bypass: &'a [String],
    /// The site TLD, always bypassed.
    pub tld: &'a str,
    /// The daemon environment reading.
    pub env: &'a SystemProxy,
    /// The OS settings reading, with the source it represents.
    pub os: Option<(ProxySource, &'a SystemProxy)>,
    /// The cached login-shell reading.
    pub login_shell: Option<&'a SystemProxy>,
}

/// The proxy Yerd will actually use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProxy {
    /// Configured mode.
    pub mode: OutboundProxyMode,
    /// Which source won.
    pub source: ProxySource,
    /// Proxy for `http://` requests.
    pub http: Option<ProxyUrl>,
    /// Proxy for `https://` requests.
    pub https: Option<ProxyUrl>,
    /// Effective bypass list.
    pub bypass: Vec<String>,
    /// Whether the OS reading flagged a PAC script or WPAD.
    pub pac_detected: bool,
}

impl ResolvedProxy {
    /// Direct connections only.
    #[must_use]
    pub fn off() -> Self {
        Self {
            mode: OutboundProxyMode::Off,
            source: ProxySource::None,
            http: None,
            https: None,
            bypass: Vec::new(),
            pac_detected: false,
        }
    }

    /// Whether any scheme goes through a proxy.
    #[must_use]
    pub fn has_proxy(&self) -> bool {
        self.http.is_some() || self.https.is_some()
    }

    /// The proxy a probe or a status line should name: HTTPS first, else HTTP.
    #[must_use]
    pub fn primary(&self) -> Option<&ProxyUrl> {
        self.https.as_ref().or(self.http.as_ref())
    }
}

/// Resolves the proxy for `input.mode`. System mode takes the first non-empty
/// reading among env, OS, and login shell; manual HTTPS falls back to the HTTP
/// URL.
#[must_use]
pub fn resolve(input: &ResolveInput<'_>) -> ResolvedProxy {
    match input.mode {
        OutboundProxyMode::Off => ResolvedProxy::off(),
        OutboundProxyMode::Manual => {
            let http = input.manual_http.cloned();
            let https = input.manual_https.or(input.manual_http).cloned();
            let source = if http.is_some() || https.is_some() {
                ProxySource::Manual
            } else {
                ProxySource::None
            };
            ResolvedProxy {
                mode: OutboundProxyMode::Manual,
                source,
                http,
                https,
                bypass: bypass_list(input.tld, input.user_bypass, &[]),
                pac_detected: false,
            }
        }
        OutboundProxyMode::System => {
            let pac_detected = input.os.is_some_and(|(_, r)| r.pac_detected);
            let candidates = std::iter::once((ProxySource::DaemonEnv, Some(input.env)))
                .chain(std::iter::once(match input.os {
                    Some((src, r)) => (src, Some(r)),
                    None => (ProxySource::None, None),
                }))
                .chain(std::iter::once((
                    ProxySource::LoginShell,
                    input.login_shell,
                )));
            let winner = candidates
                .filter_map(|(src, r)| r.map(|r| (src, r)))
                .find(|(_, r)| !r.is_empty());
            match winner {
                Some((source, r)) => ResolvedProxy {
                    mode: OutboundProxyMode::System,
                    source,
                    http: r.http.clone(),
                    https: r.https.clone(),
                    bypass: bypass_list(input.tld, input.user_bypass, &r.bypass),
                    pac_detected,
                },
                None => ResolvedProxy {
                    mode: OutboundProxyMode::System,
                    source: ProxySource::None,
                    http: None,
                    https: None,
                    bypass: bypass_list(input.tld, input.user_bypass, &[]),
                    pac_detected,
                },
            }
        }
    }
}

/// Redacts the given URLs' credentials in `text`, so an error message from a
/// client builder or a subprocess can be shown safely.
///
/// Only URL forms are touched: the full canonical URL, and any `user:password@`
/// userinfo with the password as stored, percent-encoded, or percent-decoded
/// (clients re-encode userinfo when they print a URL). A bare occurrence of the
/// password elsewhere is left alone, so a short password can't mangle
/// unrelated text.
#[must_use]
pub fn scrub(text: &str, urls: &[&ProxyUrl]) -> String {
    let mut out = text.to_owned();
    for url in urls {
        out = out.replace(url.as_str(), &url.redacted());
        let (Some(user), Some(pw)) = (url.user.as_deref(), url.password.as_deref()) else {
            continue;
        };
        if pw.is_empty() {
            continue;
        }
        for user_form in encoding_forms(user) {
            let redacted = format!("{user_form}:{REDACTED}@");
            for pw_form in encoding_forms(pw) {
                out = out.replace(&format!("{user_form}:{pw_form}@"), &redacted);
            }
        }
    }
    out
}

/// `raw` as stored, percent-encoded, and percent-decoded, without duplicates.
fn encoding_forms(raw: &str) -> Vec<String> {
    let mut forms = vec![raw.to_owned()];
    for form in [Some(percent_encode_userinfo(raw)), percent_decode(raw)]
        .into_iter()
        .flatten()
    {
        if !forms.contains(&form) {
            forms.push(form);
        }
    }
    forms
}

fn percent_encode_userinfo(raw: &str) -> String {
    let mut out = String::new();
    for b in raw.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

fn percent_decode(raw: &str) -> Option<String> {
    if !raw.contains('%') {
        return None;
    }
    let bytes = raw.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            let s = std::str::from_utf8(hex).ok()?;
            out.push(u8::from_str_radix(s, 16).ok()?);
            i += 3;
        } else {
            out.push(b);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// Merges a URL submitted from a client (which only ever saw the redacted
/// form) with the stored one.
///
/// An empty string clears; a `***` password with an unchanged scheme, user,
/// host and port keeps the stored URL; anything else parses as new.
///
/// # Errors
/// [`ProxyUrlErrorReason::RedactedPasswordChanged`] when `***` is submitted
/// for a changed endpoint or with nothing stored, so a literal `***` is never
/// stored; any parse error otherwise.
pub fn merge_submitted(
    submitted: &str,
    stored: Option<&ProxyUrl>,
) -> Result<Option<ProxyUrl>, CoreError> {
    if submitted.trim().is_empty() {
        return Ok(None);
    }
    let parsed = ProxyUrl::parse(submitted)?;
    if parsed.password.as_deref() != Some(REDACTED) {
        return Ok(Some(parsed));
    }
    match stored {
        Some(s) if s.has_password() && s.same_endpoint(&parsed) => Ok(Some(s.clone())),
        _ => Err(CoreError::InvalidProxyUrl {
            reason: ProxyUrlErrorReason::RedactedPasswordChanged,
        }),
    }
}

/// Environment changes for a child process that reaches the network.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChildEnvPlan {
    /// Variables to set.
    pub set: Vec<(String, String)>,
    /// Variables to remove.
    pub remove: Vec<String>,
}

/// Plans a child's proxy environment.
///
/// With a proxy, both cases of each proxied scheme, the bypass list, and
/// [`NODE_USE_ENV_PROXY`] are set, and both cases of a scheme with no proxy are
/// removed so an inherited value can't route it elsewhere. Off removes every [`PROXY_ENV_NAMES`]
/// entry so an inherited proxy cannot leak in. Otherwise the plan is empty.
#[must_use]
pub fn child_env_plan(resolved: &ResolvedProxy) -> ChildEnvPlan {
    if resolved.mode == OutboundProxyMode::Off {
        return ChildEnvPlan {
            set: Vec::new(),
            remove: PROXY_ENV_NAMES.iter().map(|n| (*n).to_owned()).collect(),
        };
    }
    if !resolved.has_proxy() {
        return ChildEnvPlan::default();
    }
    let mut set = Vec::new();
    let mut pair = |lower: &str, upper: &str, value: &str| {
        set.push((lower.to_owned(), value.to_owned()));
        set.push((upper.to_owned(), value.to_owned()));
    };
    let mut remove = Vec::new();
    for (url, lower, upper) in [
        (&resolved.http, "http_proxy", "HTTP_PROXY"),
        (&resolved.https, "https_proxy", "HTTPS_PROXY"),
    ] {
        match url {
            Some(u) => pair(lower, upper, u.as_str()),
            None => remove.extend([lower.to_owned(), upper.to_owned()]),
        }
    }
    pair("no_proxy", "NO_PROXY", &resolved.bypass.join(","));
    set.push((NODE_USE_ENV_PROXY.to_owned(), "1".to_owned()));
    ChildEnvPlan { set, remove }
}

/// Renders the plan's `set` entries as `KEY=VALUE` lines for `network.env`.
/// An empty result means the file should not exist.
#[must_use]
pub fn render_env_file(plan: &ChildEnvPlan) -> String {
    plan.set.iter().fold(String::new(), |mut out, (k, v)| {
        out.push_str(k);
        out.push('=');
        out.push_str(v);
        out.push('\n');
        out
    })
}

/// Parses `KEY=VALUE` lines, keeping only keys in `allowed` (first wins).
/// Blank lines, `#` comments, and malformed lines are skipped.
#[must_use]
pub fn parse_env_file(text: &str, allowed: &[&str]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() || line.trim_start().starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        if allowed.contains(&key) && !out.iter().any(|(k, _)| k == key) {
            out.push((key.to_owned(), value.to_owned()));
        }
    }
    out
}

/// The variables that name a proxy (as opposed to the bypass list).
const PROXY_URL_ENV_NAMES: [&str; 6] = [
    "http_proxy",
    "HTTP_PROXY",
    "https_proxy",
    "HTTPS_PROXY",
    "all_proxy",
    "ALL_PROXY",
];

/// The entries a shim should add to its child. `caller` returns the caller's
/// non-empty value for a variable.
///
/// When the caller sets any proxy URL (`http_proxy`, `https_proxy`,
/// `all_proxy`, either case) its own proxy wins and nothing is added. When it
/// sets only `no_proxy`/`NO_PROXY`, Yerd's proxy is still added, and both
/// bypass variables become the caller's entries followed by Yerd's
/// (de-duplicated), so loopback and the site TLD stay bypassed. Otherwise every
/// entry of `file` is added.
#[must_use]
pub fn shim_fill_in(
    file: &[(String, String)],
    caller: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    if file.is_empty() || PROXY_URL_ENV_NAMES.iter().any(|n| caller(n).is_some()) {
        return Vec::new();
    }
    let Some(theirs) = caller("no_proxy").or_else(|| caller("NO_PROXY")) else {
        return file.to_vec();
    };
    let ours = file
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("no_proxy"))
        .map(|(_, v)| v.as_str())
        .unwrap_or_default();
    let mut merged: Vec<&str> = Vec::new();
    for entry in theirs.split(',').chain(ours.split(',')).map(str::trim) {
        if !entry.is_empty() && !merged.contains(&entry) {
            merged.push(entry);
        }
    }
    let merged = merged.join(",");
    let mut out: Vec<(String, String)> = file
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("no_proxy"))
        .cloned()
        .collect();
    out.push(("no_proxy".to_owned(), merged.clone()));
    out.push(("NO_PROXY".to_owned(), merged));
    out
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

    fn url(s: &str) -> ProxyUrl {
        ProxyUrl::parse(s).unwrap()
    }

    fn reason(s: &str) -> ProxyUrlErrorReason {
        match ProxyUrl::parse(s) {
            Err(CoreError::InvalidProxyUrl { reason }) => reason,
            other => panic!("expected InvalidProxyUrl for {s:?}, got {other:?}"),
        }
    }

    #[test]
    fn parse_canonicalises() {
        for (input, canonical) in [
            ("proxy.corp:3128", "http://proxy.corp:3128"),
            ("http://proxy.corp", "http://proxy.corp:80"),
            ("https://proxy.corp", "https://proxy.corp:443"),
            ("HTTP://Proxy.Corp:8080/", "http://proxy.corp:8080"),
            ("http://u:p@proxy:8080", "http://u:p@proxy:8080"),
            ("http://u@proxy:8080", "http://u@proxy:8080"),
            ("http://u%40x:p%3Aw@proxy:1", "http://u%40x:p%3Aw@proxy:1"),
            ("http://10.0.0.1:3128", "http://10.0.0.1:3128"),
            ("http://[::1]:3128", "http://[::1]:3128"),
            ("  http://proxy:1  ", "http://proxy:1"),
        ] {
            assert_eq!(url(input).as_str(), canonical, "{input}");
        }
    }

    #[test]
    fn parse_rejects_with_reason() {
        for (input, expected) in [
            ("", ProxyUrlErrorReason::Empty),
            ("   ", ProxyUrlErrorReason::Empty),
            (
                "socks5://proxy:1080",
                ProxyUrlErrorReason::UnsupportedScheme,
            ),
            ("ftp://proxy", ProxyUrlErrorReason::UnsupportedScheme),
            ("http://proxy:1/path", ProxyUrlErrorReason::HasPathOrQuery),
            ("http://proxy:1?q", ProxyUrlErrorReason::HasPathOrQuery),
            ("http://proxy:1#f", ProxyUrlErrorReason::HasPathOrQuery),
            ("http://@proxy:1", ProxyUrlErrorReason::InvalidUserinfo),
            ("http://:pw@proxy:1", ProxyUrlErrorReason::InvalidUserinfo),
            ("http://u p@proxy:1", ProxyUrlErrorReason::InvalidUserinfo),
            ("http://:8080", ProxyUrlErrorReason::MissingHost),
            ("http://bad host:1", ProxyUrlErrorReason::InvalidHost),
            ("http://proxy:0", ProxyUrlErrorReason::InvalidPort),
            ("http://proxy:abc", ProxyUrlErrorReason::InvalidPort),
            ("http://proxy:70000", ProxyUrlErrorReason::InvalidPort),
            ("http://::1:80", ProxyUrlErrorReason::InvalidPort),
        ] {
            assert_eq!(reason(input), expected, "{input}");
        }
    }

    #[test]
    fn error_never_echoes_input() {
        let e = ProxyUrl::parse("socks5://u:hunter2@proxy").unwrap_err();
        assert!(!e.to_string().contains("hunter2"));
    }

    #[test]
    fn redaction_and_accessors() {
        let u = url("http://alice:s3cret@proxy:8080");
        assert_eq!(u.redacted(), "http://alice:***@proxy:8080");
        assert_eq!(u.user(), Some("alice"));
        assert!(u.has_password());
        assert_eq!(u.host(), "proxy");
        assert_eq!(u.port(), 8080);
        assert!(!u.secure());
        let dbg = format!("{u:?}");
        assert!(!dbg.contains("s3cret"), "{dbg}");
        assert!(dbg.contains("alice:***@proxy:8080"), "{dbg}");
        let plain = url("https://proxy");
        assert_eq!(plain.redacted(), "https://proxy:443");
        assert!(!plain.has_password());
        assert_eq!(url("http://bob@p:1").redacted(), "http://bob@p:1");
    }

    #[test]
    fn mode_round_trips() {
        for m in [
            OutboundProxyMode::System,
            OutboundProxyMode::Manual,
            OutboundProxyMode::Off,
        ] {
            assert_eq!(m.as_str().parse::<OutboundProxyMode>(), Ok(m));
        }
        assert_eq!(
            "auto".parse::<OutboundProxyMode>(),
            Err(CoreError::InvalidProxyMode {
                input: "auto".to_owned()
            })
        );
        assert_eq!(OutboundProxyMode::default(), OutboundProxyMode::System);
    }

    #[test]
    fn source_labels_are_non_empty() {
        for s in [
            ProxySource::Manual,
            ProxySource::DaemonEnv,
            ProxySource::MacosSystemSettings,
            ProxySource::GnomeSettings,
            ProxySource::LoginShell,
            ProxySource::None,
        ] {
            assert!(!s.label().is_empty());
        }
    }

    fn lookup(vars: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<String> {
        move |k| {
            vars.iter()
                .find(|(n, _)| *n == k)
                .map(|(_, v)| (*v).to_owned())
        }
    }

    #[test]
    fn env_lookup_cases() {
        let r = from_env_lookup(&lookup(&[
            ("http_proxy", "http://lower:1"),
            ("HTTP_PROXY", "http://upper:1"),
        ]));
        assert_eq!(r.http.unwrap().host(), "lower");
        assert!(r.https.is_none());

        let r = from_env_lookup(&lookup(&[
            ("HTTPS_PROXY", "http://s:2"),
            ("ALL_PROXY", "http://all:3"),
        ]));
        assert_eq!(r.https.unwrap().host(), "s");
        assert_eq!(r.http.unwrap().host(), "all");

        let r = from_env_lookup(&lookup(&[
            ("all_proxy", "socks5://s:1080"),
            ("http_proxy", "not a url"),
        ]));
        assert!(r.is_empty());

        let r = from_env_lookup(&lookup(&[
            ("http_proxy", ""),
            ("HTTP_PROXY", "http://upper:1"),
        ]));
        assert_eq!(r.http.unwrap().host(), "upper");

        let r = from_env_lookup(&lookup(&[(
            "no_proxy",
            "corp.com, *.svc ,<local>,10.0.0.0/8",
        )]));
        assert_eq!(r.bypass, vec!["corp.com", ".svc", "10.0.0.0/8"]);
        assert!(!r.pac_detected);
    }

    #[test]
    fn bypass_normalisation_table() {
        for (input, expected) in [
            ("*.corp.com", Some(".corp.com")),
            ("corp.com", Some("corp.com")),
            ("registry.corp.com", Some("registry.corp.com")),
            (".corp.com", Some(".corp.com")),
            ("CORP.COM", Some("corp.com")),
            ("*corp.com", None),
            ("a.*.com", None),
            ("corp..com", None),
            ("intranet", Some("intranet")),
            ("*.local", Some(".local")),
            ("169.254/16", Some("169.254.0.0/16")),
            ("10/8", Some("10.0.0.0/8")),
            ("127.0.0.0/8", Some("127.0.0.0/8")),
            ("fe80::/10", Some("fe80::/10")),
            ("192.168.1.1", Some("192.168.1.1")),
            ("::1", Some("::1")),
            ("[::1]", Some("::1")),
            ("host.corp:8080", Some("host.corp")),
            ("10.0.0.1:80", Some("10.0.0.1")),
            ("*", Some("*")),
            ("", None),
            ("   ", None),
            ("<local>", None),
            ("<LOCAL>", None),
            ("bad host", None),
            ("1.2.3.4/99", None),
            ("x/y", None),
        ] {
            assert_eq!(
                normalize_bypass_entry(input).as_deref(),
                expected,
                "{input:?}"
            );
        }
    }

    #[test]
    fn bypass_list_orders_and_dedupes() {
        let list = bypass_list(
            "test",
            &["corp.com".into(), "localhost".into()],
            &["*.corp.com".into(), "<local>".into(), "10/8".into()],
        );
        assert_eq!(
            list,
            vec![
                "localhost",
                "127.0.0.1",
                "127.0.0.0/8",
                "::1",
                ".test",
                "corp.com",
                ".corp.com",
                "10.0.0.0/8"
            ]
        );
    }

    #[test]
    fn bypasses_table() {
        let list: Vec<String> = [
            "localhost",
            "127.0.0.0/8",
            "::1",
            ".test",
            "corp.com",
            "10.1.2.3",
            "fd00::/8",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        for (host, expected) in [
            ("localhost", true),
            ("LOCALHOST.", true),
            ("127.4.5.6", true),
            ("128.0.0.1", false),
            ("[::1]", true),
            ("::2", false),
            ("foo.test", true),
            ("test", true),
            ("footest", false),
            ("corp.com", true),
            ("registry.corp.com", true),
            ("notcorp.com", false),
            ("10.1.2.3", true),
            ("10.1.2.4", false),
            ("fd12::1", true),
            ("files.yerd.app", false),
        ] {
            assert_eq!(bypasses(host, &list), expected, "{host}");
        }
        assert!(bypasses("anything.example", &["*".to_owned()]));
        assert!(!bypasses("x", &[]));
        assert!(!bypasses("10.0.0.1", &["10.0.0.0/99".to_owned()]));
        assert!(bypasses("1.2.3.4", &["0.0.0.0/0".to_owned()]));
    }

    #[test]
    fn normalize_bypass_entries_dedupes_in_order() {
        let entries: Vec<String> = ["b.com", "*.a.com", "B.COM", "<local>", ".a.com", "x y"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        assert_eq!(normalize_bypass_entries(&entries), vec!["b.com", ".a.com"]);
        assert!(normalize_bypass_entries(&[]).is_empty());
    }

    fn sys(http: Option<&str>, https: Option<&str>, pac: bool) -> SystemProxy {
        SystemProxy {
            http: http.map(url),
            https: https.map(url),
            bypass: vec![".sys.corp".into()],
            pac_detected: pac,
        }
    }

    fn input<'a>(
        mode: OutboundProxyMode,
        env: &'a SystemProxy,
        os: Option<&'a SystemProxy>,
        shell: Option<&'a SystemProxy>,
    ) -> ResolveInput<'a> {
        ResolveInput {
            mode,
            manual_http: None,
            manual_https: None,
            user_bypass: &[],
            tld: "test",
            env,
            os: os.map(|r| (ProxySource::MacosSystemSettings, r)),
            login_shell: shell,
        }
    }

    #[test]
    fn system_precedence() {
        let empty = SystemProxy::default();
        let env = sys(Some("http://env:1"), None, false);
        let os = sys(Some("http://os:1"), Some("http://os:2"), false);
        let shell = sys(None, Some("http://shell:1"), false);
        for (e, o, s, source, host) in [
            (&env, Some(&os), Some(&shell), ProxySource::DaemonEnv, "env"),
            (
                &empty,
                Some(&os),
                Some(&shell),
                ProxySource::MacosSystemSettings,
                "os",
            ),
            (&empty, None, Some(&shell), ProxySource::LoginShell, "shell"),
            (
                &empty,
                Some(&empty),
                Some(&shell),
                ProxySource::LoginShell,
                "shell",
            ),
        ] {
            let r = resolve(&input(OutboundProxyMode::System, e, o, s));
            assert_eq!(r.source, source);
            assert_eq!(r.primary().unwrap().host(), host);
        }
        let r = resolve(&input(OutboundProxyMode::System, &empty, None, None));
        assert_eq!(r.source, ProxySource::None);
        assert!(!r.has_proxy());
        assert!(r.bypass.contains(&".test".to_owned()));
    }

    #[test]
    fn system_merges_source_bypass() {
        let empty = SystemProxy::default();
        let os = sys(Some("http://os:1"), None, false);
        let r = resolve(&input(OutboundProxyMode::System, &empty, Some(&os), None));
        assert!(r.bypass.contains(&".sys.corp".to_owned()));
    }

    #[test]
    fn pac_flag_comes_from_os_reading() {
        let empty = SystemProxy::default();
        let pac_only = sys(None, None, true);
        let r = resolve(&input(
            OutboundProxyMode::System,
            &empty,
            Some(&pac_only),
            None,
        ));
        assert!(r.pac_detected);
        assert!(!r.has_proxy());
        let pac_static = sys(Some("http://os:1"), None, true);
        let r = resolve(&input(
            OutboundProxyMode::System,
            &empty,
            Some(&pac_static),
            None,
        ));
        assert!(r.pac_detected);
        assert!(r.has_proxy());
    }

    #[test]
    fn manual_https_falls_back_to_http() {
        let empty = SystemProxy::default();
        let http = url("http://u:p@m:8080");
        let mut i = input(OutboundProxyMode::Manual, &empty, None, None);
        i.manual_http = Some(&http);
        let user = vec!["corp.com".to_owned()];
        i.user_bypass = &user;
        let r = resolve(&i);
        assert_eq!(r.source, ProxySource::Manual);
        assert_eq!(r.https.as_ref(), Some(&http));
        assert_eq!(r.http.as_ref(), Some(&http));
        assert!(r.bypass.contains(&"corp.com".to_owned()));
        let https = url("http://s:1");
        i.manual_https = Some(&https);
        assert_eq!(resolve(&i).https.as_ref(), Some(&https));
    }

    #[test]
    fn manual_without_urls_has_no_source() {
        let empty = SystemProxy::default();
        let r = resolve(&input(OutboundProxyMode::Manual, &empty, None, None));
        assert_eq!(r.source, ProxySource::None);
    }

    #[test]
    fn off_ignores_every_source() {
        let env = sys(Some("http://env:1"), None, true);
        let r = resolve(&input(OutboundProxyMode::Off, &env, Some(&env), Some(&env)));
        assert_eq!(r, ResolvedProxy::off());
    }

    #[test]
    fn scrub_redacts_url_userinfo_only() {
        let u = url("http://u:hunter2@p:1");
        let text = "error connecting to http://u:hunter2@p:1 via u:hunter2@other";
        let s = scrub(text, &[&u]);
        assert!(!s.contains("hunter2"), "{s}");
        assert!(s.contains("http://u:***@p:1"), "{s}");
        assert!(s.contains("u:***@other"), "{s}");

        let short = url("http://u:a@p:1");
        assert_eq!(scrub("a bad gateway", &[&short]), "a bad gateway");

        let special = url("http://u:p%40ss@p:1");
        for text in [
            "proxy http://u:p%40ss@p:1/ failed",
            "proxy u:p@ss@p:1 failed",
        ] {
            let s = scrub(text, &[&special]);
            assert!(s.contains("u:***@"), "{s}");
            assert!(!s.contains("p%40ss") && !s.contains("p@ss"), "{s}");
        }
        let plain = url("http://u:p@ss@p:1");
        let s = scrub("proxy http://u:p%40ss@p:1 failed", &[&plain]);
        assert!(s.contains("u:***@"), "{s}");

        let encoded_user = url("http://a%40corp:pw1@p:1");
        for text in [
            "proxy http://a%40corp:pw1@p:1 failed",
            "proxy a@corp:pw1@p:1 failed",
        ] {
            let s = scrub(text, &[&encoded_user]);
            assert!(!s.contains("pw1"), "{s}");
        }
        let raw_user = url("http://a.b@corp:pw2@p:1");
        let s = scrub("proxy http://a.b%40corp:pw2@p:1 failed", &[&raw_user]);
        assert!(!s.contains("pw2"), "{s}");

        let none = url("http://p:1");
        assert_eq!(scrub("x", &[&none]), "x");
    }

    #[test]
    fn merge_submitted_cases() {
        let stored = url("http://u:real@host:8080");
        assert_eq!(merge_submitted("", Some(&stored)), Ok(None));
        assert_eq!(merge_submitted("  ", None), Ok(None));
        assert_eq!(
            merge_submitted("http://u:***@host:8080", Some(&stored)),
            Ok(Some(stored.clone()))
        );
        assert_eq!(
            merge_submitted("http://u:new@host:8080", Some(&stored)),
            Ok(Some(url("http://u:new@host:8080")))
        );
        let changed = CoreError::InvalidProxyUrl {
            reason: ProxyUrlErrorReason::RedactedPasswordChanged,
        };
        for submitted in [
            "http://u:***@other:8080",
            "http://u:***@host:9090",
            "https://u:***@host:8080",
            "http://v:***@host:8080",
        ] {
            assert_eq!(
                merge_submitted(submitted, Some(&stored)),
                Err(changed.clone()),
                "{submitted}"
            );
        }
        assert_eq!(
            merge_submitted("http://u:***@host:8080", None),
            Err(changed.clone())
        );
        let no_pw = url("http://u@host:8080");
        assert_eq!(
            merge_submitted("http://u:***@host:8080", Some(&no_pw)),
            Err(changed)
        );
        assert!(merge_submitted("socks5://x", None).is_err());
    }

    fn resolved_manual(http: &str) -> ResolvedProxy {
        ResolvedProxy {
            mode: OutboundProxyMode::Manual,
            source: ProxySource::Manual,
            http: Some(url(http)),
            https: Some(url(http)),
            bypass: vec!["localhost".into(), ".test".into()],
            pac_detected: false,
        }
    }

    #[test]
    fn child_env_plan_with_proxy() {
        let plan = child_env_plan(&resolved_manual("http://u:p@m:1"));
        let get = |k: &str| {
            plan.set
                .iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.as_str())
        };
        for k in ["http_proxy", "HTTP_PROXY", "https_proxy", "HTTPS_PROXY"] {
            assert_eq!(get(k), Some("http://u:p@m:1"), "{k}");
        }
        assert_eq!(get("no_proxy"), Some("localhost,.test"));
        assert_eq!(get("NO_PROXY"), Some("localhost,.test"));
        assert_eq!(get(NODE_USE_ENV_PROXY), Some("1"));
        assert_eq!(plan.set.len(), 7);
        assert!(plan.remove.is_empty());
    }

    #[test]
    fn child_env_plan_http_only_sets_http_pair() {
        let mut r = resolved_manual("http://m:1");
        r.https = None;
        let plan = child_env_plan(&r);
        assert!(plan
            .set
            .iter()
            .all(|(k, _)| !k.eq_ignore_ascii_case("https_proxy")));
        assert_eq!(plan.set.len(), 5);
        assert_eq!(plan.remove, vec!["https_proxy", "HTTPS_PROXY"]);
        let mut https_only = resolved_manual("http://m:1");
        https_only.http = None;
        assert_eq!(
            child_env_plan(&https_only).remove,
            vec!["http_proxy", "HTTP_PROXY"]
        );
    }

    #[test]
    fn child_env_plan_off_and_none() {
        let off = child_env_plan(&ResolvedProxy::off());
        assert!(off.set.is_empty());
        assert_eq!(off.remove.len(), 8);
        assert!(off.remove.contains(&"all_proxy".to_owned()));
        let mut none = ResolvedProxy::off();
        none.mode = OutboundProxyMode::System;
        assert_eq!(child_env_plan(&none), ChildEnvPlan::default());
    }

    #[test]
    fn env_file_round_trips() {
        let plan = child_env_plan(&resolved_manual("http://u:p@m:1"));
        let text = render_env_file(&plan);
        let mut allowed: Vec<&str> = PROXY_ENV_NAMES.to_vec();
        allowed.push(NODE_USE_ENV_PROXY);
        assert_eq!(parse_env_file(&text, &allowed), plan.set);
        assert!(render_env_file(&ChildEnvPlan::default()).is_empty());
    }

    #[test]
    fn parse_env_file_filters() {
        let text = "# c\n\nhttp_proxy=http://a:1\r\nEVIL=x\nnoequals\nhttp_proxy=http://b:1\nNODE_USE_ENV_PROXY=1\n";
        assert_eq!(
            parse_env_file(text, &PROXY_ENV_NAMES),
            vec![("http_proxy".to_owned(), "http://a:1".to_owned())]
        );
    }

    #[test]
    fn shim_fill_in_cases() {
        let file = vec![
            ("http_proxy".to_owned(), "http://m:1".to_owned()),
            (NODE_USE_ENV_PROXY.to_owned(), "1".to_owned()),
        ];
        assert_eq!(shim_fill_in(&file, &|_| None), file);
        for has in [
            "http_proxy",
            "HTTP_PROXY",
            "https_proxy",
            "HTTPS_PROXY",
            "all_proxy",
            "ALL_PROXY",
        ] {
            let caller = |k: &str| (k == has).then(|| "http://mine:1".to_owned());
            assert!(shim_fill_in(&file, &caller).is_empty(), "{has}");
        }
        let caller = |k: &str| (k == NODE_USE_ENV_PROXY).then(|| "0".to_owned());
        assert_eq!(shim_fill_in(&file, &caller).len(), 2);
        assert!(shim_fill_in(&[], &|_| None).is_empty());
    }

    #[test]
    fn shim_fill_in_merges_a_caller_bypass_list() {
        let file = vec![
            ("HTTPS_PROXY".to_owned(), "http://m:1".to_owned()),
            ("no_proxy".to_owned(), "localhost,.test".to_owned()),
            ("NO_PROXY".to_owned(), "localhost,.test".to_owned()),
            (NODE_USE_ENV_PROXY.to_owned(), "1".to_owned()),
        ];
        for name in ["no_proxy", "NO_PROXY"] {
            let caller = |k: &str| (k == name).then(|| "corp.com, localhost".to_owned());
            let got = shim_fill_in(&file, &caller);
            assert_eq!(
                got,
                vec![
                    ("HTTPS_PROXY".to_owned(), "http://m:1".to_owned()),
                    (NODE_USE_ENV_PROXY.to_owned(), "1".to_owned()),
                    ("no_proxy".to_owned(), "corp.com,localhost,.test".to_owned()),
                    ("NO_PROXY".to_owned(), "corp.com,localhost,.test".to_owned()),
                ],
                "{name}"
            );
        }
    }
}
