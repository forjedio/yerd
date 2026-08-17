//! Pure composition + matching for the Windows NRPT (`Name Resolution Policy
//! Table`) `.test` wildcard rule.
//!
//! Compiled on every OS so Linux/macOS CI table-tests it too (the "decisions in
//! pure helpers" rule). No I/O: these functions only build the single-cmdlet
//! PowerShell bodies the elevated helper runs, and match one registry rule's
//! decoded values.
//!
//! ## Registry ground truth
//!
//! The shapes the code below depends on:
//!
//! - Rules live under
//!   `HKLM\SYSTEM\CurrentControlSet\Services\Dnscache\Parameters\DnsPolicyConfig`.
//! - Each rule is a **subkey** whose name is a GUID **with braces**, e.g.
//!   `{E16C9E47-6AAA-41BB-9219-CDBB6BF37AE7}`. That braced subkey name is exactly
//!   the value `Remove-DnsClientNrptRule -Name '<guid>'` expects (confirmed via
//!   `Get-DnsClientNrptRule`, whose `Name` column is the same braced GUID).
//! - Value shapes inside a rule subkey:
//!     - `Name`               :: `REG_MULTI_SZ` :: `[".test"]` (the namespace, dot kept)
//!     - `GenericDNSServers`  :: `REG_SZ`       :: `127.0.0.1`
//!     - `ConfigOptions`      :: `REG_DWORD`    :: `8`
//!     - `Version`            :: `REG_DWORD`    :: `2`
//!     - `Comment`/`DisplayName`/`IPSECCARestriction` :: `REG_SZ` :: empty
//! - `Get-DnsClientNrptRule` works **unelevated** (so does reading the HKLM key).
//! - `Add-DnsClientNrptRule` resolves via module autoload (`ModuleName=DnsClient`).
//! - `powershell.exe` absolute path present at
//!   `%SystemRoot%\System32\WindowsPowerShell\v1.0\powershell.exe`.
//! - `winreg 0.55`: `Vec<String>: FromRegValue` decodes `REG_MULTI_SZ` (`Name`);
//!   `String: FromRegValue` decodes `REG_SZ` (`GenericDNSServers`).

use std::net::Ipv4Addr;

use yerd_core::Tld;

/// Remove any single-quote byte so a defective input can never break out of the
/// `'...'` quoting in the composed cmdlet. Valid inputs (a validated [`Tld`], an
/// [`Ipv4Addr`] display, a registry GUID) never contain one, so this is a no-op
/// for every real value and a hard backstop otherwise.
fn strip_quotes(raw: &str) -> String {
    raw.chars().filter(|c| *c != '\'').collect()
}

/// The NRPT namespace for `tld`: a leading-dot wildcard, e.g. `test` -> `.test`.
#[must_use]
pub fn namespace(tld: &Tld) -> String {
    format!(".{}", tld.as_str())
}

/// The single-cmdlet body that creates the wildcard rule routing `.tld` at `ip`.
///
/// `Add-` (not `Set-`) is the creating cmdlet; `Set-` only edits an existing
/// rule. One cmdlet, discrete literal args, no pipeline or second statement.
#[must_use]
pub fn add_rule_cmd(tld: &Tld, ip: Ipv4Addr) -> String {
    let ns = strip_quotes(&namespace(tld));
    let ip = strip_quotes(&ip.to_string());
    debug_assert!(!ns.contains('\''));
    debug_assert!(!ip.contains('\''));
    format!("Add-DnsClientNrptRule -Namespace '{ns}' -NameServers '{ip}' -Comment 'yerd'")
}

/// The single-cmdlet body that deletes the rule identified by `guid` (the braced
/// GUID read from our own registry enumeration, never user input).
#[must_use]
pub fn remove_rule_cmd(guid: &str) -> String {
    let guid = strip_quotes(guid);
    debug_assert!(!guid.contains('\''));
    format!("Remove-DnsClientNrptRule -Name '{guid}' -Force")
}

/// The single-cmdlet body that drops cached negative answers after a rule change.
#[must_use]
pub fn flush_cmd() -> &'static str {
    "Clear-DnsClientCache"
}

/// Whether a rule's decoded `Name` multi-sz names the `.tld` namespace.
///
/// Used for GUID discovery (remove/replace any `.tld` rule regardless of the
/// server it points at). Case-insensitive, whitespace-trimmed.
#[must_use]
pub fn name_matches_tld(name_entries: &[String], tld: &str) -> bool {
    let want = format!(".{tld}");
    name_entries
        .iter()
        .any(|entry| entry.trim().eq_ignore_ascii_case(&want))
}

/// The individual addresses in a rule's `GenericDNSServers` `REG_SZ`.
///
/// The value holds one or more addresses separated by `;`, `,`, or whitespace.
/// Entries are trimmed and empty ones dropped, so a trailing separator or a
/// value that is entirely blank yields an empty vec rather than blank strings.
///
/// Shared by [`rule_matches`] (the `is_installed` probe) and the doctor-facing
/// `nrpt_servers_for_tld` reader, so both read a rule's targets the same way.
#[must_use]
pub fn split_servers(servers: &str) -> Vec<String> {
    servers
        .split([';', ',', ' ', '\t'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

/// Whether a rule both names the `.tld` namespace and forwards to `ip`.
///
/// The `is_installed` probe's matcher: the rule must name `.tld` AND its
/// `GenericDNSServers` must list `ip` (a rule aimed elsewhere reports absent so
/// the redirect is re-installed). The server list is read with
/// [`split_servers`], so an empty `ip` never matches a blank entry.
#[must_use]
pub fn rule_matches(name_entries: &[String], servers: &str, tld: &str, ip: &str) -> bool {
    name_matches_tld(name_entries, tld) && split_servers(servers).iter().any(|s| s == ip)
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

    fn tld(s: &str) -> Tld {
        Tld::new(s).unwrap()
    }

    #[test]
    fn add_rule_cmd_is_single_cmdlet_with_leading_dot_namespace() {
        let cmd = add_rule_cmd(&tld("test"), Ipv4Addr::LOCALHOST);
        assert_eq!(
            cmd,
            "Add-DnsClientNrptRule -Namespace '.test' -NameServers '127.0.0.1' -Comment 'yerd'"
        );
        assert!(!cmd.contains('|'), "no pipeline");
        assert!(!cmd.contains(';'), "no second statement");
    }

    #[test]
    fn add_rule_cmd_multi_label_tld() {
        let cmd = add_rule_cmd(&tld("dev.local"), Ipv4Addr::LOCALHOST);
        assert!(cmd.contains("-Namespace '.dev.local'"), "{cmd}");
    }

    #[test]
    fn remove_rule_cmd_uses_braced_guid_and_force() {
        let cmd = remove_rule_cmd("{E16C9E47-6AAA-41BB-9219-CDBB6BF37AE7}");
        assert_eq!(
            cmd,
            "Remove-DnsClientNrptRule -Name '{E16C9E47-6AAA-41BB-9219-CDBB6BF37AE7}' -Force"
        );
    }

    #[test]
    fn flush_cmd_is_clear_cache() {
        assert_eq!(flush_cmd(), "Clear-DnsClientCache");
    }

    #[test]
    fn compose_strips_defensive_quotes() {
        assert_eq!(
            remove_rule_cmd("a'b').whatever"),
            "Remove-DnsClientNrptRule -Name 'ab).whatever' -Force",
            "injected single quotes must be removed so the value cannot break out"
        );
    }

    #[test]
    fn name_matches_tld_exact_and_case_insensitive() {
        assert!(name_matches_tld(&[".test".to_owned()], "test"));
        assert!(name_matches_tld(&[".TEST".to_owned()], "test"));
        assert!(name_matches_tld(&["  .test ".to_owned()], "test"));
        assert!(!name_matches_tld(&[".other".to_owned()], "test"));
        assert!(
            !name_matches_tld(&["test".to_owned()], "test"),
            "needs the dot"
        );
        assert!(!name_matches_tld(&[], "test"));
    }

    #[test]
    fn rule_matches_full_probe() {
        let name = vec![".test".to_owned()];
        assert!(rule_matches(&name, "127.0.0.1", "test", "127.0.0.1"));
        assert!(
            !rule_matches(&name, "127.0.0.53", "test", "127.0.0.1"),
            "wrong server must not match"
        );
        assert!(
            !rule_matches(&[".other".to_owned()], "127.0.0.1", "test", "127.0.0.1"),
            "wrong tld must not match"
        );
        assert!(
            !rule_matches(&[], "127.0.0.1", "test", "127.0.0.1"),
            "empty name must not match"
        );
    }

    #[test]
    fn split_servers_table() {
        let cases: &[(&str, &[&str])] = &[
            ("", &[]),
            ("   ", &[]),
            ("127.0.0.1", &["127.0.0.1"]),
            ("10.0.0.1;127.0.0.1", &["10.0.0.1", "127.0.0.1"]),
            ("10.0.0.1,127.0.0.1", &["10.0.0.1", "127.0.0.1"]),
            ("10.0.0.1 127.0.0.1", &["10.0.0.1", "127.0.0.1"]),
            ("10.0.0.1\t127.0.0.1", &["10.0.0.1", "127.0.0.1"]),
            (
                " 10.0.0.1 ;\t127.0.0.1 , 8.8.8.8 ",
                &["10.0.0.1", "127.0.0.1", "8.8.8.8"],
            ),
            (";;127.0.0.1;;", &["127.0.0.1"]),
        ];
        for (raw, want) in cases {
            assert_eq!(split_servers(raw), *want, "input {raw:?}");
        }
    }

    #[test]
    fn split_servers_never_yields_blanks() {
        assert!(
            !rule_matches(&[".test".to_owned()], ";;", "test", ""),
            "an empty ip must not match a blank server entry"
        );
    }

    #[test]
    fn rule_matches_multi_namespace_and_multi_server() {
        let name = vec![".other".to_owned(), ".test".to_owned()];
        assert!(rule_matches(
            &name,
            "10.0.0.1;127.0.0.1",
            "test",
            "127.0.0.1"
        ));
    }
}
