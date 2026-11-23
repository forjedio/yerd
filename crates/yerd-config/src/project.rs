//! The committed per-project file, `.yerd.toml`.
//!
//! A small, user-facing schema (not a copy of `yerd.toml`) that a project
//! commits so a teammate can run `yerd init` after a clone. This module is
//! pure: parse with typed [`ProjectError`]s, validate, render deterministically,
//! export a site's state into a [`ProjectFile`], apply a write-back
//! [`SyncDelta`], and plan an apply. [`ProjectFile::load`] and
//! [`ProjectFile::save`] are the only I/O and live in `io.rs`.
//!
//! The file stores domains as TLD-less sub-parts so it stays portable across
//! machines with a different TLD. Comments a user adds are not preserved when
//! Yerd rewrites the file.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::str::FromStr;

use serde::Deserialize;
use yerd_core::{
    normalize_site_name, Domain, PhpVersion, ProxyRule, ResponseHeader, UpstreamTarget,
};

use crate::error::ProjectError;
use crate::parse::KNOWN_SINGLE_SERVICES;

/// The project file's name, in a project's root directory.
pub const PROJECT_FILE_NAME: &str = ".yerd.toml";

/// The largest `.yerd.toml` [`ProjectFile::load`] reads, in bytes.
pub const PROJECT_FILE_MAX_BYTES: u64 = 64 * 1024;

/// The service version value that matches any installed or available version.
pub const ANY_SERVICE_VERSION: &str = "*";

const HEADER_COMMENT: &str = "# Yerd project file. Commit it, then teammates run `yerd init`.\n\
                              # Reference: https://yerd.app/reference/project-file\n";

/// A parsed, validated `.yerd.toml`.
///
/// Collections are kept in canonical order (domains and extensions sorted,
/// headers by case-insensitive name, rules by prefix), so two files that render
/// identically compare equal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProjectFile {
    /// The site name (`<name>.<tld>`); `None` means the folder slug.
    pub name: Option<String>,
    /// The site's PHP version; `None` leaves the site's PHP alone.
    pub php: Option<PhpVersion>,
    /// Whether the site is served over HTTPS; `None` leaves it as is.
    pub secure: Option<bool>,
    /// The Node release selector; `None` means Yerd does not manage Node here.
    pub node: Option<String>,
    /// Extra domains as TLD-less sub-parts (`admin.acme`, `*.acme`).
    pub domains: Vec<Domain>,
    /// Required PHP module names, lowercase.
    pub extensions: Vec<String>,
    /// Custom response headers.
    pub headers: Vec<ResponseHeader>,
    /// Path-prefix proxy rules.
    pub proxy_rules: Vec<ProxyRule>,
    /// Required services: engine id to a version prefix or `"*"`.
    pub services: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    name: Option<String>,
    php: Option<String>,
    secure: Option<bool>,
    node: Option<String>,
    #[serde(default)]
    domains: Vec<String>,
    #[serde(default)]
    extensions: Vec<String>,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    proxy_rules: BTreeMap<String, String>,
    #[serde(default)]
    services: BTreeMap<String, String>,
}

fn invalid(field: &'static str, reason: impl ToString) -> ProjectError {
    ProjectError::Invalid {
        field,
        reason: reason.to_string(),
    }
}

impl ProjectFile {
    /// Parses and validates `.yerd.toml` text. Unknown keys are rejected.
    ///
    /// # Errors
    /// [`ProjectError::Parse`] for malformed TOML, a wrong type or an unknown
    /// key; [`ProjectError::Invalid`] naming the field for a value that fails
    /// validation.
    pub fn from_toml(s: &str) -> Result<Self, ProjectError> {
        let wire: Wire = toml::from_str(s)?;
        let mut file = Self::try_from_wire(wire)?;
        file.canonicalise();
        file.validate()?;
        Ok(file)
    }

    fn try_from_wire(wire: Wire) -> Result<Self, ProjectError> {
        let name = wire
            .name
            .map(|raw| {
                normalize_site_name(&raw)
                    .ok_or_else(|| invalid("name", format!("{raw:?} is not a valid site name")))
            })
            .transpose()?;
        let php = wire
            .php
            .map(|raw| PhpVersion::from_str(&raw).map_err(|e| invalid("php", e)))
            .transpose()?;
        let domains = wire
            .domains
            .iter()
            .map(|raw| Domain::parse_subpart(raw).map_err(|e| invalid("domains", e)))
            .collect::<Result<Vec<_>, _>>()?;
        let headers = wire
            .headers
            .iter()
            .map(|(k, v)| ResponseHeader::new(k, v).map_err(|e| invalid("headers", e)))
            .collect::<Result<Vec<_>, _>>()?;
        let proxy_rules = wire
            .proxy_rules
            .iter()
            .map(|(prefix, url)| {
                UpstreamTarget::from_url_str(url)
                    .and_then(|target| ProxyRule::new(prefix, target))
                    .map_err(|e| invalid("proxy_rules", e))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            name,
            php,
            secure: wire.secure,
            node: wire.node.map(|n| n.trim().to_owned()),
            domains,
            extensions: wire
                .extensions
                .iter()
                .map(|e| e.trim().to_ascii_lowercase())
                .collect(),
            headers,
            proxy_rules,
            services: wire.services,
        })
    }

    /// Checks the invariants a hand-built value must meet to render and parse
    /// back: a valid site name and Node selector, no duplicate domain, header
    /// name (ignoring case), rule prefix or extension, extension names in
    /// `[a-z0-9_]+`, and services that are known engines with a `"*"` or dotted
    /// version prefix.
    ///
    /// # Errors
    /// [`ProjectError::Invalid`] naming the first offending field.
    pub fn validate(&self) -> Result<(), ProjectError> {
        if let Some(name) = &self.name {
            if normalize_site_name(name).as_deref() != Some(name.as_str()) {
                return Err(invalid(
                    "name",
                    format!("{name:?} is not a valid site name"),
                ));
            }
        }
        if let Some(node) = &self.node {
            yerd_core::node::validate_selector(node).map_err(|e| invalid("node", e))?;
        }
        if has_duplicate(self.domains.iter().map(Domain::as_str)) {
            return Err(invalid("domains", "a domain is listed twice"));
        }
        validate_extensions(&self.extensions)?;
        if has_duplicate(self.headers.iter().map(|h| h.name().to_ascii_lowercase())) {
            return Err(invalid(
                "headers",
                "a header name is listed twice (names ignore case)",
            ));
        }
        if has_duplicate(self.proxy_rules.iter().map(ProxyRule::prefix)) {
            return Err(invalid("proxy_rules", "two rules share a path prefix"));
        }
        validate_services(&self.services)
    }

    /// Checks that no declared domain already carries the live TLD: apply
    /// appends `.<tld>`, so `admin.acme.test` would become `admin.acme.test.test`.
    ///
    /// # Errors
    /// [`ProjectError::Invalid`] on `domains` naming the first such domain.
    pub fn validate_for_tld(&self, tld: &str) -> Result<(), ProjectError> {
        match self.domains.iter().find(|d| ends_with_tld(d, tld)) {
            Some(domain) => Err(invalid(
                "domains",
                format!(
                    "{} already ends in .{tld}; list domains without the TLD",
                    domain.as_str()
                ),
            )),
            None => Ok(()),
        }
    }

    fn canonicalise(&mut self) {
        self.domains.sort();
        self.domains.dedup();
        self.extensions.sort();
        self.extensions.dedup();
        self.headers.sort_by(compare_headers);
        self.proxy_rules.sort_by(|a, b| a.prefix().cmp(b.prefix()));
    }

    /// Renders the file: a two-line header comment, then scalars, arrays and
    /// tables in a fixed order with sorted keys. Deterministic for equal values.
    #[must_use]
    pub fn to_toml(&self) -> String {
        let mut canonical = self.clone();
        canonical.canonicalise();
        canonical.render()
    }

    fn render(&self) -> String {
        let mut top = String::new();
        if let Some(name) = &self.name {
            let _ = writeln!(top, "name = {}", quote(name));
        }
        if let Some(php) = self.php {
            let _ = writeln!(top, "php = {}", quote(&php.to_string()));
        }
        if let Some(secure) = self.secure {
            let _ = writeln!(top, "secure = {secure}");
        }
        if let Some(node) = &self.node {
            let _ = writeln!(top, "node = {}", quote(node));
        }
        if !self.domains.is_empty() {
            let _ = writeln!(
                top,
                "domains = {}",
                array(self.domains.iter().map(Domain::as_str))
            );
        }
        if !self.extensions.is_empty() {
            let _ = writeln!(
                top,
                "extensions = {}",
                array(self.extensions.iter().map(String::as_str))
            );
        }

        let mut out = String::from(HEADER_COMMENT);
        if !top.is_empty() {
            out.push('\n');
            out.push_str(&top);
        }
        render_table(
            &mut out,
            "headers",
            self.headers
                .iter()
                .map(|h| (h.name().to_owned(), h.value().to_owned())),
        );
        render_table(
            &mut out,
            "proxy_rules",
            self.proxy_rules
                .iter()
                .map(|r| (r.prefix().to_owned(), r.target().to_string())),
        );
        render_table(
            &mut out,
            "services",
            self.services.iter().map(|(k, v)| (k.clone(), v.clone())),
        );
        out
    }

    /// Builds the file for a site's current state, for export and the wizard
    /// draft. PHP, HTTPS, domains, headers and rules come from `snapshot`;
    /// `name`, `services` and `extensions` are kept from `existing`, and `name`
    /// is filled from the snapshot only when absent. The existing `node`
    /// selector is kept when `node_matches` (the caller checked the stored
    /// preference is an installed release the selector matches); otherwise the
    /// stored exact version is written.
    #[must_use]
    pub fn from_snapshot(
        snapshot: &SiteSnapshot,
        existing: Option<&ProjectFile>,
        node_matches: bool,
    ) -> Self {
        let mut file = existing.cloned().unwrap_or_default();
        if file.name.is_none() {
            file.name = Some(snapshot.name.clone());
        }
        file.php = Some(snapshot.php);
        file.secure = Some(snapshot.secure);
        let apex = Domain::apex(&snapshot.name);
        file.domains = snapshot
            .domains_added
            .iter()
            .filter(|d| **d != apex)
            .cloned()
            .collect();
        file.headers.clone_from(&snapshot.headers);
        file.proxy_rules.clone_from(&snapshot.proxy_rules);
        file.node = match (node_matches, file.node.take()) {
            (true, Some(selector)) => Some(selector),
            _ => snapshot.node_pref.clone(),
        };
        file.canonicalise();
        file
    }

    /// Reads and parses the project file at `path`. Returns `Ok(None)` when
    /// nothing is there. A symlink or other non-regular file is refused, as is
    /// anything over [`PROJECT_FILE_MAX_BYTES`].
    ///
    /// # Errors
    /// [`ProjectError::NotRegularFile`], [`ProjectError::TooLarge`],
    /// [`ProjectError::Io`], or a parse/validation error.
    pub fn load(path: &std::path::Path) -> Result<Option<Self>, ProjectError> {
        crate::io::load_project(path)
    }

    /// Renders and writes the file atomically: a temp file in the same
    /// directory, mode 0644 on Unix, then a rename. The directory must exist,
    /// and an existing non-regular file at `path` is refused.
    ///
    /// # Errors
    /// [`ProjectError::NotRegularFile`] or [`ProjectError::Io`].
    pub fn save(&self, path: &std::path::Path) -> Result<(), ProjectError> {
        crate::io::save_project(self, path)
    }

    /// Applies one write-back change and reports whether the model changed.
    /// Entries the delta does not name are untouched, so a declared but not yet
    /// applied entry survives.
    pub fn apply_delta(&mut self, delta: SyncDelta) -> bool {
        let before = self.clone();
        match delta {
            SyncDelta::Php(v) => self.php = Some(v),
            SyncDelta::Secure(b) => self.secure = Some(b),
            SyncDelta::Node(selector) => self.node = selector,
            SyncDelta::AddDomain(d) => self.domains.push(d),
            SyncDelta::RemoveDomain(d) => self.domains.retain(|x| *x != d),
            SyncDelta::ResetDomains => self.domains.clear(),
            SyncDelta::SetHeader(h) => {
                self.headers.retain(|x| !x.has_name(h.name()));
                self.headers.push(h);
            }
            SyncDelta::RemoveHeader(name) => self.headers.retain(|x| !x.has_name(&name)),
            SyncDelta::SetRule(rule) => {
                self.proxy_rules.retain(|r| r.prefix() != rule.prefix());
                self.proxy_rules.push(rule);
            }
            SyncDelta::RemoveRule(prefix) => {
                let prefix = normalise_prefix(&prefix);
                self.proxy_rules.retain(|r| r.prefix() != prefix);
            }
        }
        self.canonicalise();
        *self != before
    }
}

fn compare_headers(a: &ResponseHeader, b: &ResponseHeader) -> Ordering {
    a.name()
        .to_ascii_lowercase()
        .cmp(&b.name().to_ascii_lowercase())
        .then_with(|| a.name().cmp(b.name()))
}

fn normalise_prefix(prefix: &str) -> &str {
    let trimmed = prefix.trim_end_matches('/');
    if trimmed.is_empty() && prefix.starts_with('/') {
        "/"
    } else {
        trimmed
    }
}

fn has_duplicate<T: Ord>(items: impl Iterator<Item = T>) -> bool {
    let mut seen = BTreeSet::new();
    items.into_iter().any(|item| !seen.insert(item))
}

fn validate_extensions(extensions: &[String]) -> Result<(), ProjectError> {
    for ext in extensions {
        let ok = !ext.is_empty()
            && ext
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
        if !ok {
            return Err(invalid(
                "extensions",
                format!("{ext:?} is not a module name (use lowercase letters, digits and _)"),
            ));
        }
    }
    if has_duplicate(extensions.iter()) {
        return Err(invalid("extensions", "an extension is listed twice"));
    }
    Ok(())
}

fn validate_services(services: &BTreeMap<String, String>) -> Result<(), ProjectError> {
    for (engine, version) in services {
        if !KNOWN_SINGLE_SERVICES.contains(&engine.as_str()) {
            return Err(invalid(
                "services",
                format!(
                    "unknown service {engine:?} (known: {})",
                    KNOWN_SINGLE_SERVICES.join(", ")
                ),
            ));
        }
        if !is_version_prefix(version) {
            return Err(invalid(
                "services",
                format!("{engine} version {version:?} must be \"*\" or a version such as \"8.4\""),
            ));
        }
    }
    Ok(())
}

fn is_version_prefix(version: &str) -> bool {
    version == ANY_SERVICE_VERSION
        || version.split('.').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        })
}

fn render_table(out: &mut String, title: &str, entries: impl Iterator<Item = (String, String)>) {
    let mut body = String::new();
    for (key, value) in entries {
        let _ = writeln!(body, "{} = {}", key_repr(&key), quote(&value));
    }
    if !body.is_empty() {
        let _ = write!(out, "\n[{title}]\n{body}");
    }
}

fn array<'a>(items: impl Iterator<Item = &'a str>) -> String {
    let quoted: Vec<String> = items.map(quote).collect();
    format!("[{}]", quoted.join(", "))
}

fn key_repr(key: &str) -> String {
    let bare = !key.is_empty()
        && key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if bare {
        key.to_owned()
    } else {
        quote(key)
    }
}

fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                let _ = write!(out, "\\u{:04X}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// A site's current state as plain owned data, filled by the daemon for
/// [`ProjectFile::from_snapshot`] and [`plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SiteSnapshot {
    /// The site's name.
    pub name: String,
    /// The site's effective PHP version.
    pub php: PhpVersion,
    /// Whether the site is served over HTTPS.
    pub secure: bool,
    /// The domains the site's delta adds (TLD-less sub-parts).
    pub domains_added: Vec<Domain>,
    /// Whether the site's delta suppresses its default apex `<name>`, so the
    /// apex is not served until it is added back.
    pub apex_suppressed: bool,
    /// The site's custom response headers.
    pub headers: Vec<ResponseHeader>,
    /// The site's path proxy rules, excluding service-managed ones.
    pub proxy_rules: Vec<ProxyRule>,
    /// The stored exact Node version preference for the project, if any.
    pub node_pref: Option<String>,
}

/// One write-back change derived from a successful request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncDelta {
    /// Set `php`.
    Php(PhpVersion),
    /// Set `secure`.
    Secure(bool),
    /// Set `node` to the given selector, or remove it.
    Node(Option<String>),
    /// Add a domain.
    AddDomain(Domain),
    /// Remove a domain.
    RemoveDomain(Domain),
    /// Clear `domains`.
    ResetDomains,
    /// Insert a header, replacing one with the same name (ignoring case).
    SetHeader(ResponseHeader),
    /// Remove the header with this name (ignoring case).
    RemoveHeader(String),
    /// Insert a rule, replacing one with the same prefix.
    SetRule(ProxyRule),
    /// Remove the rule with this prefix.
    RemoveRule(String),
}

/// What one planned apply step does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlannedAction {
    /// Link the folder, under the file's name (`None` means the folder slug).
    Link {
        /// The declared site name, if any.
        name: Option<String>,
    },
    /// Set the site's PHP version.
    Php(PhpVersion),
    /// Set HTTPS on or off.
    Secure(bool),
    /// Add a domain.
    Domain(Domain),
    /// Set a response header.
    Header(ResponseHeader),
    /// Add a path rule, or update its upstream when `update`.
    ProxyRule {
        /// The declared rule.
        rule: ProxyRule,
        /// The site already has this prefix with another upstream.
        update: bool,
    },
    /// Pin the Node selector for the project.
    Node(String),
}

/// One apply step and whether it changes anything; unchanged steps are still
/// listed so apply can report them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedStep {
    /// The step.
    pub action: PlannedAction,
    /// Whether the step changes the site.
    pub needed: bool,
}

/// Plans the site-scoped part of applying `file` to the site in `snapshot`
/// (`None` when the folder is not a site yet). Order: link, PHP, HTTPS,
/// domains, headers, path rules, Node. `node_unchanged` is the caller's check
/// that the stored preference is an installed release the selector matches. A
/// declared apex counts as present only while the snapshot does not suppress it.
#[must_use]
pub fn plan(
    file: &ProjectFile,
    snapshot: Option<&SiteSnapshot>,
    node_unchanged: bool,
) -> Vec<PlannedStep> {
    let step = |action, needed| PlannedStep { action, needed };
    let mut steps = vec![step(
        PlannedAction::Link {
            name: file.name.clone(),
        },
        snapshot.is_none(),
    )];
    if let Some(v) = file.php {
        steps.push(step(
            PlannedAction::Php(v),
            snapshot.map_or(true, |s| s.php != v),
        ));
    }
    if let Some(b) = file.secure {
        steps.push(step(
            PlannedAction::Secure(b),
            snapshot.map_or(true, |s| s.secure != b),
        ));
    }
    let apex = snapshot
        .map(|s| s.name.as_str())
        .or(file.name.as_deref())
        .map(Domain::apex);
    let apex_served = !snapshot.is_some_and(|s| s.apex_suppressed);
    for d in &file.domains {
        let present = (apex_served && apex.as_ref() == Some(d))
            || snapshot.is_some_and(|s| s.domains_added.contains(d));
        steps.push(step(PlannedAction::Domain(d.clone()), !present));
    }
    for h in &file.headers {
        let present = snapshot.is_some_and(|s| {
            s.headers
                .iter()
                .any(|x| x.has_name(h.name()) && x.value() == h.value())
        });
        steps.push(step(PlannedAction::Header(h.clone()), !present));
    }
    for rule in &file.proxy_rules {
        let current =
            snapshot.and_then(|s| s.proxy_rules.iter().find(|r| r.prefix() == rule.prefix()));
        let (update, needed) = match current {
            None => (false, true),
            Some(r) if r.target() == rule.target() => (false, false),
            Some(_) => (true, true),
        };
        steps.push(step(
            PlannedAction::ProxyRule {
                rule: rule.clone(),
                update,
            },
            needed,
        ));
    }
    if let Some(selector) = &file.node {
        steps.push(step(PlannedAction::Node(selector.clone()), !node_unchanged));
    }
    steps
}

/// Whether service version `label` (an exact published label such as `8.4.9`)
/// matches `prefix` component-wise: `"8.4"` matches `8.4.9` but not `8.40.1`,
/// `"8"` matches `8.0.2`, and `"*"` matches anything.
#[must_use]
pub fn service_prefix_matches(prefix: &str, label: &str) -> bool {
    if prefix == ANY_SERVICE_VERSION {
        return true;
    }
    if prefix.is_empty() {
        return false;
    }
    let mut parts = label.split('.');
    prefix.split('.').all(|want| parts.next() == Some(want))
}

/// What apply does for one declared service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceAction {
    /// A matching version is installed; `start` when it is not running.
    Satisfied {
        /// The service must be started.
        start: bool,
    },
    /// Nothing is installed: install this label, the newest matching one.
    Install(String),
    /// Only versions outside the declared prefix are installed.
    Mismatch,
    /// Nothing is installed and no available version matches.
    Unavailable,
}

/// Decides how to satisfy a declared service from its `installed` and
/// `available` version labels. When the instance has a `selected` version, only
/// that one counts: another matching version on disk is not the one it runs.
/// An installed or selected version outside `prefix` is never switched
/// automatically.
#[must_use]
pub fn service_action(
    prefix: &str,
    installed: &[String],
    selected: Option<&str>,
    available: &[String],
    running: bool,
) -> ServiceAction {
    if let Some(selected) = selected {
        return if service_prefix_matches(prefix, selected) {
            ServiceAction::Satisfied { start: !running }
        } else {
            ServiceAction::Mismatch
        };
    }
    if installed.iter().any(|l| service_prefix_matches(prefix, l)) {
        return ServiceAction::Satisfied { start: !running };
    }
    if !installed.is_empty() {
        return ServiceAction::Mismatch;
    }
    available
        .iter()
        .filter(|l| service_prefix_matches(prefix, l))
        .max_by(|a, b| compare_labels(a, b))
        .map_or(ServiceAction::Unavailable, |l| {
            ServiceAction::Install(l.clone())
        })
}

fn compare_labels(a: &str, b: &str) -> Ordering {
    let mut left = a.split('.');
    let mut right = b.split('.');
    loop {
        match (left.next(), right.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(nx), Ok(ny)) => nx.cmp(&ny),
                    _ => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Whether domain sub-part `domain` ends in `.<tld>` (ignoring case), so it
/// was written with the TLD that Yerd appends itself.
#[must_use]
pub fn ends_with_tld(domain: &Domain, tld: &str) -> bool {
    let tld = tld.trim_matches('.');
    if tld.is_empty() {
        return false;
    }
    domain
        .as_str()
        .to_ascii_lowercase()
        .ends_with(&format!(".{}", tld.to_ascii_lowercase()))
}

/// Normalises a PHP module name for comparison: trimmed, lowercased, spaces to
/// `_`, and a leading `zend_` dropped, so `Zend OPcache` becomes `opcache`.
#[must_use]
pub fn normalise_module(name: &str) -> String {
    let lowered = name.trim().to_ascii_lowercase().replace(' ', "_");
    match lowered.strip_prefix("zend_") {
        Some(rest) if !rest.is_empty() => rest.to_owned(),
        _ => lowered,
    }
}

/// Whether required extension `name` is available for a PHP version: compiled
/// into the build (`compiled`, raw `php -m` names), registered as a custom
/// extension (`custom` names), or `xdebug` with Xdebug enabled.
#[must_use]
pub fn extension_satisfied(
    name: &str,
    compiled: &BTreeSet<String>,
    custom: &[String],
    xdebug_on: bool,
) -> bool {
    let wanted = normalise_module(name);
    (wanted == "xdebug" && xdebug_on)
        || compiled.iter().any(|m| normalise_module(m) == wanted)
        || custom.iter().any(|m| normalise_module(m) == wanted)
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

    fn php(s: &str) -> PhpVersion {
        s.parse().unwrap()
    }

    fn dom(s: &str) -> Domain {
        Domain::parse_subpart(s).unwrap()
    }

    fn header(n: &str, v: &str) -> ResponseHeader {
        ResponseHeader::new(n, v).unwrap()
    }

    fn rule(prefix: &str, url: &str) -> ProxyRule {
        ProxyRule::new(prefix, UpstreamTarget::from_url_str(url).unwrap()).unwrap()
    }

    fn labels(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    type ServiceCase = (
        &'static str,
        Vec<String>,
        Option<&'static str>,
        Vec<String>,
        bool,
        ServiceAction,
    );
    type DeltaCase = (SyncDelta, bool, Box<dyn Fn(&ProjectFile) -> bool>);

    fn snapshot() -> SiteSnapshot {
        SiteSnapshot {
            name: "acme".to_owned(),
            php: php("8.4"),
            secure: true,
            domains_added: vec![dom("admin.acme")],
            apex_suppressed: false,
            headers: vec![header("X-Frame-Options", "DENY")],
            proxy_rules: vec![rule("/app", "http://127.0.0.1:3000")],
            node_pref: Some("22.11.0".to_owned()),
        }
    }

    fn file_for_snapshot() -> ProjectFile {
        ProjectFile {
            name: Some("acme".to_owned()),
            php: Some(php("8.4")),
            secure: Some(true),
            node: Some("22".to_owned()),
            domains: vec![dom("admin.acme")],
            headers: vec![header("x-frame-options", "DENY")],
            proxy_rules: vec![rule("/app", "http://127.0.0.1:3000")],
            ..ProjectFile::default()
        }
    }

    #[test]
    fn from_toml_rejects_invalid_input_naming_the_field() {
        let cases: &[(&str, Option<&str>)] = &[
            ("php = \"8\"", Some("php")),
            ("php = \"banana\"", Some("php")),
            ("name = \"---\"", Some("name")),
            ("node = \"latest\"", Some("node")),
            ("domains = [\"a..b\"]", Some("domains")),
            ("domains = [\"acme.*\"]", Some("domains")),
            ("[headers]\nX-A = \"1\"\nx-a = \"2\"", Some("headers")),
            ("[headers]\nHost = \"x\"", Some("headers")),
            (
                "[proxy_rules]\n\"app\" = \"http://127.0.0.1:3000\"",
                Some("proxy_rules"),
            ),
            ("[proxy_rules]\n\"/app\" = \"ftp://x\"", Some("proxy_rules")),
            (
                "[proxy_rules]\n\"/app\" = \"http://a:1\"\n\"/app/\" = \"http://b:1\"",
                Some("proxy_rules"),
            ),
            ("[services]\nmongodb = \"7\"", Some("services")),
            ("[services]\nreverb = \"*\"", Some("services")),
            ("[services]\nmysql = \"\"", Some("services")),
            ("[services]\nmysql = \"8..4\"", Some("services")),
            ("extensions = [\"Zend OPcache\"]", Some("extensions")),
            ("extensions = [\"\"]", Some("extensions")),
            ("colour = \"blue\"", None),
            ("secure = \"yes\"", None),
            ("domains = \"acme\"", None),
            ("not toml at all", None),
        ];
        for (input, field) in cases {
            let err = ProjectFile::from_toml(input).unwrap_err();
            match (field, &err) {
                (Some(want), ProjectError::Invalid { field, .. }) => {
                    assert_eq!(field, want, "{input:?}");
                }
                (None, ProjectError::Parse(_)) => {}
                _ => panic!("{input:?} gave unexpected {err:?}"),
            }
        }
    }

    #[test]
    fn unknown_key_error_names_the_key() {
        let err = ProjectFile::from_toml("colour = \"blue\"").unwrap_err();
        assert!(err.to_string().contains("colour"), "{err}");
    }

    #[test]
    fn from_toml_normalises_values() {
        let cases: &[(&str, &str)] = &[
            ("name = \"Acme\"", "name = \"acme\""),
            ("php = \"php8.3\"", "php = \"8.3\""),
            ("node = \" 22 \"", "node = \"22\""),
            (
                "domains = [\"b.acme\", \"A.acme\", \"b.acme\"]",
                "domains = [\"a.acme\", \"b.acme\"]",
            ),
            (
                "extensions = [\"redis\", \"Imagick\", \"Redis\"]",
                "extensions = [\"imagick\", \"redis\"]",
            ),
            (
                "[proxy_rules]\n\"/app/\" = \"http://localhost\"",
                "\"/app\" = \"http://localhost:80\"",
            ),
        ];
        for (input, want) in cases {
            let rendered = ProjectFile::from_toml(input).unwrap().to_toml();
            assert!(rendered.contains(want), "{input:?} rendered {rendered}");
        }
    }

    #[test]
    fn empty_file_is_default() {
        assert_eq!(ProjectFile::from_toml("").unwrap(), ProjectFile::default());
        assert_eq!(ProjectFile::default().to_toml(), HEADER_COMMENT);
    }

    #[test]
    fn render_quotes_awkward_keys_and_values() {
        let file = ProjectFile {
            headers: vec![header("X-Quote", "say \"hi\" \\ there")],
            ..ProjectFile::default()
        };
        let rendered = file.to_toml();
        assert!(
            rendered.contains(r#"X-Quote = "say \"hi\" \\ there""#),
            "{rendered}"
        );
        assert_eq!(ProjectFile::from_toml(&rendered).unwrap(), file);
    }

    #[test]
    fn validate_rejects_hand_built_invalid_values() {
        let cases: Vec<(ProjectFile, &str)> = vec![
            (
                ProjectFile {
                    name: Some("Acme".to_owned()),
                    ..ProjectFile::default()
                },
                "name",
            ),
            (
                ProjectFile {
                    node: Some("nope".to_owned()),
                    ..ProjectFile::default()
                },
                "node",
            ),
            (
                ProjectFile {
                    domains: vec![dom("a.acme"), dom("a.acme")],
                    ..ProjectFile::default()
                },
                "domains",
            ),
            (
                ProjectFile {
                    headers: vec![header("X-A", "1"), header("x-a", "2")],
                    ..ProjectFile::default()
                },
                "headers",
            ),
            (
                ProjectFile {
                    proxy_rules: vec![rule("/a", "http://x:1"), rule("/a", "http://y:1")],
                    ..ProjectFile::default()
                },
                "proxy_rules",
            ),
            (
                ProjectFile {
                    extensions: vec!["Redis".to_owned()],
                    ..ProjectFile::default()
                },
                "extensions",
            ),
        ];
        for (file, want) in cases {
            match file.validate() {
                Err(ProjectError::Invalid { field, .. }) => assert_eq!(field, want),
                other => panic!("{file:?} gave {other:?}"),
            }
        }
        file_for_snapshot().validate().unwrap();
    }

    #[test]
    fn plan_new_site_needs_every_step() {
        let steps = plan(&file_for_snapshot(), None, false);
        assert_eq!(steps.len(), 7);
        assert!(steps.iter().all(|s| s.needed), "{steps:?}");
        assert_eq!(
            steps[0].action,
            PlannedAction::Link {
                name: Some("acme".to_owned())
            }
        );
    }

    #[test]
    fn plan_identical_snapshot_needs_nothing() {
        let steps = plan(&file_for_snapshot(), Some(&snapshot()), true);
        assert_eq!(steps.len(), 7);
        assert!(steps.iter().all(|s| !s.needed), "{steps:?}");
    }

    #[test]
    fn plan_reports_each_difference() {
        let mut file = file_for_snapshot();
        file.php = Some(php("8.3"));
        file.secure = Some(false);
        file.domains = vec![dom("*.acme"), dom("acme"), dom("admin.acme")];
        file.headers = vec![header("X-FRAME-OPTIONS", "SAMEORIGIN")];
        file.proxy_rules = vec![
            rule("/app", "http://127.0.0.1:4000"),
            rule("/ws", "http://127.0.0.1:6001"),
        ];
        let got: Vec<(PlannedAction, bool)> = plan(&file, Some(&snapshot()), false)
            .into_iter()
            .map(|s| (s.action, s.needed))
            .collect();
        let want = vec![
            (
                PlannedAction::Link {
                    name: Some("acme".to_owned()),
                },
                false,
            ),
            (PlannedAction::Php(php("8.3")), true),
            (PlannedAction::Secure(false), true),
            (PlannedAction::Domain(dom("*.acme")), true),
            (PlannedAction::Domain(dom("acme")), false),
            (PlannedAction::Domain(dom("admin.acme")), false),
            (
                PlannedAction::Header(header("X-FRAME-OPTIONS", "SAMEORIGIN")),
                true,
            ),
            (
                PlannedAction::ProxyRule {
                    rule: rule("/app", "http://127.0.0.1:4000"),
                    update: true,
                },
                true,
            ),
            (
                PlannedAction::ProxyRule {
                    rule: rule("/ws", "http://127.0.0.1:6001"),
                    update: false,
                },
                true,
            ),
            (PlannedAction::Node("22".to_owned()), true),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn plan_apex_domain_respects_a_suppressed_apex() {
        let cases: &[(Option<bool>, bool)] =
            &[(None, false), (Some(false), false), (Some(true), true)];
        for (suppressed, want) in cases {
            let file = ProjectFile {
                name: Some("acme".to_owned()),
                domains: vec![dom("acme")],
                ..ProjectFile::default()
            };
            let snap = suppressed.map(|apex_suppressed| SiteSnapshot {
                apex_suppressed,
                ..snapshot()
            });
            let needed = plan(&file, snap.as_ref(), false)
                .into_iter()
                .find(|s| s.action == PlannedAction::Domain(dom("acme")))
                .map(|s| s.needed);
            assert_eq!(needed, Some(*want), "{suppressed:?}");
        }
    }

    #[test]
    fn plan_omits_undeclared_scalars() {
        let file = ProjectFile::default();
        let steps = plan(&file, Some(&snapshot()), false);
        assert_eq!(
            steps,
            vec![PlannedStep {
                action: PlannedAction::Link { name: None },
                needed: false
            }]
        );
    }

    #[test]
    fn service_prefix_matching() {
        let cases: &[(&str, &str, bool)] = &[
            ("8.4", "8.4.9", true),
            ("8.4", "8.40.1", false),
            ("8.4", "8.5.0", false),
            ("8", "8.0.2", true),
            ("8", "80.0.2", false),
            ("8.4.9", "8.4.9", true),
            ("8.4.9", "8.4", false),
            ("17", "17.10", true),
            ("*", "1.2.3", true),
            ("*", "anything", true),
            ("", "8.4.9", false),
        ];
        for (prefix, label, want) in cases {
            assert_eq!(
                service_prefix_matches(prefix, label),
                *want,
                "{prefix} {label}"
            );
        }
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn service_action_matrix() {
        let available = labels(&["8.0.42", "8.4.2", "8.4.10", "9.1.0"]);
        let cases: Vec<ServiceCase> = vec![
            (
                "8.4",
                vec![],
                None,
                available.clone(),
                false,
                ServiceAction::Install("8.4.10".to_owned()),
            ),
            (
                "*",
                vec![],
                None,
                available.clone(),
                false,
                ServiceAction::Install("9.1.0".to_owned()),
            ),
            (
                "7",
                vec![],
                None,
                available.clone(),
                false,
                ServiceAction::Unavailable,
            ),
            (
                "8.4",
                vec![],
                None,
                vec![],
                false,
                ServiceAction::Unavailable,
            ),
            (
                "8.4",
                labels(&["8.4.2"]),
                None,
                available.clone(),
                true,
                ServiceAction::Satisfied { start: false },
            ),
            (
                "8",
                labels(&["8.4.2"]),
                None,
                available.clone(),
                false,
                ServiceAction::Satisfied { start: true },
            ),
            (
                "8.4",
                labels(&["8.0.42"]),
                None,
                available.clone(),
                false,
                ServiceAction::Mismatch,
            ),
            (
                "8.4",
                labels(&["8.0.42", "8.4.2"]),
                Some("8.0.42"),
                available.clone(),
                true,
                ServiceAction::Mismatch,
            ),
            (
                "8.4",
                labels(&["8.0.42", "8.4.2"]),
                Some("8.4.2"),
                available.clone(),
                false,
                ServiceAction::Satisfied { start: true },
            ),
            (
                "*",
                labels(&["8.0.42"]),
                Some("8.0.42"),
                available.clone(),
                true,
                ServiceAction::Satisfied { start: false },
            ),
            (
                "8.4",
                vec![],
                Some("8.0.42"),
                available.clone(),
                false,
                ServiceAction::Mismatch,
            ),
        ];
        for (prefix, installed, selected, avail, running, want) in cases {
            assert_eq!(
                service_action(prefix, &installed, selected, &avail, running),
                want,
                "{prefix} {installed:?} {selected:?}"
            );
        }
    }

    #[test]
    fn normalise_module_cases() {
        let cases: &[(&str, &str)] = &[
            ("Zend OPcache", "opcache"),
            ("redis", "redis"),
            ("  Imagick ", "imagick"),
            ("FFI", "ffi"),
            ("Xdebug", "xdebug"),
            ("zend_", "zend_"),
            ("pdo_mysql", "pdo_mysql"),
        ];
        for (input, want) in cases {
            assert_eq!(normalise_module(input), *want, "{input:?}");
        }
    }

    #[test]
    fn ends_with_tld_cases() {
        let cases: &[(&str, &str, bool)] = &[
            ("admin.acme.test", "test", true),
            ("*.acme.test", "test", true),
            ("admin.acme", "test", false),
            ("test", "test", false),
            ("acme.testing", "test", false),
            ("admin.acme.local", "test", false),
            ("admin.acme.test", "", false),
        ];
        for (domain, tld, want) in cases {
            let parsed = Domain::parse_subpart(domain).unwrap();
            assert_eq!(ends_with_tld(&parsed, tld), *want, "{domain} {tld}");
        }
    }

    #[test]
    fn validate_for_tld_rejects_a_domain_with_the_tld() {
        let ok = ProjectFile::from_toml("domains = [\"admin.acme\"]\n").unwrap();
        assert!(ok.validate_for_tld("test").is_ok());
        let bad = ProjectFile::from_toml("domains = [\"admin.acme.test\"]\n").unwrap();
        let err = bad.validate_for_tld("test").unwrap_err();
        assert!(
            matches!(&err, ProjectError::Invalid { field: "domains", reason } if reason.contains("admin.acme.test")),
            "{err:?}"
        );
        assert!(bad.validate_for_tld("local").is_ok());
    }

    #[test]
    fn extension_satisfied_cases() {
        let compiled: BTreeSet<String> = ["Core", "Zend OPcache", "redis"]
            .iter()
            .map(|s| (*s).to_owned())
            .collect();
        let custom = labels(&["imagick"]);
        let cases: &[(&str, bool, bool)] = &[
            ("opcache", false, true),
            ("redis", false, true),
            ("imagick", false, true),
            ("xdebug", true, true),
            ("xdebug", false, false),
            ("swoole", true, false),
        ];
        for (name, xdebug_on, want) in cases {
            assert_eq!(
                extension_satisfied(name, &compiled, &custom, *xdebug_on),
                *want,
                "{name}"
            );
        }
    }

    #[test]
    fn apply_delta_every_variant() {
        let base = file_for_snapshot();
        let cases: Vec<DeltaCase> = vec![
            (
                SyncDelta::Php(php("8.3")),
                true,
                Box::new(|f| f.php == Some(php("8.3"))),
            ),
            (SyncDelta::Php(php("8.4")), false, Box::new(|_| true)),
            (
                SyncDelta::Secure(false),
                true,
                Box::new(|f| f.secure == Some(false)),
            ),
            (
                SyncDelta::Node(Some("20".to_owned())),
                true,
                Box::new(|f| f.node.as_deref() == Some("20")),
            ),
            (SyncDelta::Node(None), true, Box::new(|f| f.node.is_none())),
            (
                SyncDelta::AddDomain(dom("api.acme")),
                true,
                Box::new(|f| f.domains == vec![dom("admin.acme"), dom("api.acme")]),
            ),
            (
                SyncDelta::AddDomain(dom("admin.acme")),
                false,
                Box::new(|_| true),
            ),
            (
                SyncDelta::RemoveDomain(dom("admin.acme")),
                true,
                Box::new(|f| f.domains.is_empty()),
            ),
            (
                SyncDelta::RemoveDomain(dom("nope.acme")),
                false,
                Box::new(|_| true),
            ),
            (
                SyncDelta::ResetDomains,
                true,
                Box::new(|f| f.domains.is_empty()),
            ),
            (
                SyncDelta::SetHeader(header("X-Frame-Options", "SAMEORIGIN")),
                true,
                Box::new(|f| f.headers == vec![header("X-Frame-Options", "SAMEORIGIN")]),
            ),
            (
                SyncDelta::SetHeader(header("X-New", "1")),
                true,
                Box::new(|f| f.headers.len() == 2),
            ),
            (
                SyncDelta::RemoveHeader("X-FRAME-OPTIONS".to_owned()),
                true,
                Box::new(|f| f.headers.is_empty()),
            ),
            (
                SyncDelta::SetRule(rule("/app", "http://127.0.0.1:4000")),
                true,
                Box::new(|f| f.proxy_rules == vec![rule("/app", "http://127.0.0.1:4000")]),
            ),
            (
                SyncDelta::SetRule(rule("/app", "http://127.0.0.1:3000")),
                false,
                Box::new(|_| true),
            ),
            (
                SyncDelta::RemoveRule("/app/".to_owned()),
                true,
                Box::new(|f| f.proxy_rules.is_empty()),
            ),
            (
                SyncDelta::RemoveRule("/other".to_owned()),
                false,
                Box::new(|_| true),
            ),
        ];
        for (delta, changed, check) in cases {
            let mut file = base.clone();
            let label = format!("{delta:?}");
            assert_eq!(file.apply_delta(delta), changed, "{label}");
            assert!(check(&file), "{label} gave {file:?}");
            if !changed {
                assert_eq!(file, base, "{label}");
            }
        }
    }

    #[test]
    fn declared_domain_survives_unrelated_add() {
        let mut file = ProjectFile {
            domains: vec![dom("declared.acme")],
            ..ProjectFile::default()
        };
        assert!(file.apply_delta(SyncDelta::AddDomain(dom("api.acme"))));
        assert_eq!(file.domains, vec![dom("api.acme"), dom("declared.acme")]);
    }

    #[test]
    fn from_snapshot_keeps_matching_node_selector() {
        let existing = file_for_snapshot();
        let cases: &[(Option<&ProjectFile>, bool, Option<&str>)] = &[
            (Some(&existing), true, Some("22")),
            (Some(&existing), false, Some("22.11.0")),
            (None, true, Some("22.11.0")),
            (None, false, Some("22.11.0")),
        ];
        for (existing, matches, want) in cases {
            let file = ProjectFile::from_snapshot(&snapshot(), *existing, *matches);
            assert_eq!(file.node.as_deref(), *want, "{matches}");
        }
    }

    #[test]
    fn from_snapshot_replaces_site_state_and_keeps_the_rest() {
        let existing = ProjectFile {
            name: Some("declared".to_owned()),
            php: Some(php("8.1")),
            domains: vec![dom("old.acme")],
            extensions: vec!["redis".to_owned()],
            services: BTreeMap::from([("mysql".to_owned(), "8.4".to_owned())]),
            ..ProjectFile::default()
        };
        let mut snap = snapshot();
        snap.domains_added.push(dom("acme"));
        snap.node_pref = None;
        let file = ProjectFile::from_snapshot(&snap, Some(&existing), false);
        assert_eq!(file.name.as_deref(), Some("declared"));
        assert_eq!(file.php, Some(php("8.4")));
        assert_eq!(file.secure, Some(true));
        assert_eq!(file.domains, vec![dom("admin.acme")]);
        assert_eq!(file.headers, snap.headers);
        assert_eq!(file.proxy_rules, snap.proxy_rules);
        assert_eq!(file.extensions, existing.extensions);
        assert_eq!(file.services, existing.services);
        assert_eq!(file.node, None);

        let fresh = ProjectFile::from_snapshot(&snap, None, false);
        assert_eq!(fresh.name.as_deref(), Some("acme"));
        assert!(fresh.services.is_empty());
    }
}
