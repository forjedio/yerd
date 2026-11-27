//! Pure tunnel-provider model shared by the persisted config and the IPC wire.
//!
//! Yerd can share a site through three tunnel providers ([`TunnelProvider`]).
//! This module holds the provider enum, the readiness decision the daemon
//! reports for each one ([`readiness`]), and the small value helpers the
//! provider settings need: secret masking ([`mask_secret`]), static-domain
//! normalising ([`normalize_static_domain`]) and token shape checks
//! ([`is_valid_provider_token`]). It does no I/O.

use serde::{Deserialize, Serialize};

/// A tunnel provider a share can run through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TunnelProvider {
    /// Cloudflare Tunnel via `cloudflared` (quick links or a named tunnel).
    #[default]
    Cloudflare,
    /// ngrok via the `ngrok` agent.
    Ngrok,
    /// zrok via the `zrok2` binary.
    Zrok,
}

impl TunnelProvider {
    /// Every provider, in display order.
    pub const ALL: [Self; 3] = [Self::Cloudflare, Self::Ngrok, Self::Zrok];

    /// The wire and config spelling (`cloudflare`, `ngrok`, `zrok`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cloudflare => "cloudflare",
            Self::Ngrok => "ngrok",
            Self::Zrok => "zrok",
        }
    }

    /// The human-facing name (`Cloudflare`, `ngrok`, `zrok`).
    #[must_use]
    pub fn display_name(self) -> &'static str {
        match self {
            Self::Cloudflare => "Cloudflare",
            Self::Ngrok => "ngrok",
            Self::Zrok => "zrok",
        }
    }
}

/// Whether a provider can be used for a share right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderReadiness {
    /// The provider's binary is not available.
    NotInstalled,
    /// The binary is available but the provider still needs an account step.
    NeedsSetup,
    /// The provider can run shares.
    Ready,
}

/// Decides a provider's readiness from whether its binary is available and
/// whether its account step is done.
///
/// A missing binary always wins. Cloudflare is ready once installed because
/// quick links need no account; ngrok needs a saved authtoken and zrok an
/// enabled environment.
#[must_use]
pub fn readiness(provider: TunnelProvider, installed: bool, configured: bool) -> ProviderReadiness {
    if !installed {
        return ProviderReadiness::NotInstalled;
    }
    match provider {
        TunnelProvider::Cloudflare => ProviderReadiness::Ready,
        TunnelProvider::Ngrok | TunnelProvider::Zrok if configured => ProviderReadiness::Ready,
        TunnelProvider::Ngrok | TunnelProvider::Zrok => ProviderReadiness::NeedsSetup,
    }
}

const MASK: &str = "••••••••";

/// Masks a secret for display: the first 4 and last 2 characters around a
/// fixed run of dots when it has 12 or more characters, only dots otherwise.
///
/// Works on chars, so multibyte input never splits a code point.
#[must_use]
pub fn mask_secret(s: &str) -> String {
    let count = s.chars().count();
    if count < 12 {
        return MASK.to_owned();
    }
    let head: String = s.chars().take(4).collect();
    let tail: String = s.chars().skip(count - 2).collect();
    format!("{head}{MASK}{tail}")
}

/// Normalises user input for a static tunnel domain to a bare hostname.
///
/// Trims whitespace, strips one leading `https://` or `http://` (any case) and
/// trailing slashes, then lowercases ASCII. Validation stays with the config.
#[must_use]
pub fn normalize_static_domain(input: &str) -> String {
    let trimmed = input.trim();
    let without_scheme = ["https://", "http://"]
        .iter()
        .find_map(|scheme| {
            trimmed
                .get(..scheme.len())
                .filter(|head| head.eq_ignore_ascii_case(scheme))
                .and_then(|_| trimmed.get(scheme.len()..))
        })
        .unwrap_or(trimmed);
    without_scheme.trim_end_matches('/').to_ascii_lowercase()
}

/// Whether `s` has the shape of a provider token: 1 to 256 bytes of visible
/// ASCII with no whitespace, not starting with `-` (so it can never be read
/// as a flag on a command line).
#[must_use]
pub fn is_valid_provider_token(s: &str) -> bool {
    (1..=256).contains(&s.len()) && s.bytes().all(|b| b.is_ascii_graphic()) && !s.starts_with('-')
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn readiness_matrix() {
        use ProviderReadiness::{NeedsSetup, NotInstalled, Ready};
        use TunnelProvider::{Cloudflare, Ngrok, Zrok};
        for (provider, installed, configured, expected) in [
            (Cloudflare, false, false, NotInstalled),
            (Cloudflare, false, true, NotInstalled),
            (Cloudflare, true, false, Ready),
            (Cloudflare, true, true, Ready),
            (Ngrok, false, false, NotInstalled),
            (Ngrok, false, true, NotInstalled),
            (Ngrok, true, false, NeedsSetup),
            (Ngrok, true, true, Ready),
            (Zrok, false, false, NotInstalled),
            (Zrok, false, true, NotInstalled),
            (Zrok, true, false, NeedsSetup),
            (Zrok, true, true, Ready),
        ] {
            assert_eq!(
                readiness(provider, installed, configured),
                expected,
                "{provider:?} installed={installed} configured={configured}"
            );
        }
    }

    #[test]
    fn mask_secret_cases() {
        let token = "2fXkAbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdQa";
        for (input, expected) in [
            ("", "••••••••".to_owned()),
            ("abcdefghijk", "••••••••".to_owned()),
            ("abcdefghijkl", "abcd••••••••kl".to_owned()),
            (token, "2fXk••••••••Qa".to_owned()),
        ] {
            assert_eq!(mask_secret(input), expected, "{input}");
        }
    }

    #[test]
    fn mask_secret_handles_multibyte_without_leaking_the_middle() {
        let input = "ééééSECRETMIDDLEüü";
        let masked = mask_secret(input);
        assert_eq!(masked, "éééé••••••••üü");
        assert!(!masked.contains("SECRET"));
    }

    #[test]
    fn normalize_static_domain_cases() {
        for (input, expected) in [
            ("https://Foo.ngrok-free.app/", "foo.ngrok-free.app"),
            ("demo.ngrok-free.app", "demo.ngrok-free.app"),
            ("  demo.ngrok-free.app \n", "demo.ngrok-free.app"),
            ("http://demo.ngrok.app", "demo.ngrok.app"),
            ("HTTPS://Demo.Ngrok.App//", "demo.ngrok.app"),
            ("", ""),
            ("https://", ""),
        ] {
            assert_eq!(normalize_static_domain(input), expected, "{input:?}");
        }
    }

    #[test]
    fn is_valid_provider_token_cases() {
        let long = "a".repeat(257);
        let max = "a".repeat(256);
        for (input, expected) in [
            ("2fXk_abc-123", true),
            (max.as_str(), true),
            ("", false),
            (long.as_str(), false),
            ("a b", false),
            ("ab\n", false),
            ("-x", false),
            ("tökén", false),
        ] {
            assert_eq!(is_valid_provider_token(input), expected, "{input:?}");
        }
    }

    #[test]
    fn display_names_and_wire_spelling() {
        for (provider, wire, name) in [
            (TunnelProvider::Cloudflare, "cloudflare", "Cloudflare"),
            (TunnelProvider::Ngrok, "ngrok", "ngrok"),
            (TunnelProvider::Zrok, "zrok", "zrok"),
        ] {
            assert_eq!(provider.as_str(), wire);
            assert_eq!(provider.display_name(), name);
        }
        assert_eq!(TunnelProvider::default(), TunnelProvider::Cloudflare);
    }
}
