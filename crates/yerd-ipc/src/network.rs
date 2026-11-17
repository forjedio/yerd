//! Wire types for the outbound-proxy settings (see
//! [`crate::Request::GetNetworkSettings`], [`crate::Request::SetNetworkSettings`]
//! and [`crate::Request::TestNetwork`]). Every URL here is redacted: a password
//! only ever appears as `***`. Field order is the contract.

use serde::{Deserialize, Serialize};
use yerd_core::network::{OutboundProxyMode, ProxySource};

/// The proxy the daemon is actually using.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EffectiveProxy {
    /// Configured mode.
    pub mode: OutboundProxyMode,
    /// Which source won.
    pub source: ProxySource,
    /// Redacted proxy for `http://` requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_proxy: Option<String>,
    /// Redacted proxy for `https://` requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https_proxy: Option<String>,
    /// Effective bypass list, loopback and the site TLD included.
    #[serde(default)]
    pub bypass: Vec<String>,
}

/// Saved outbound-proxy settings plus what they resolve to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkSettings {
    /// Configured mode.
    pub mode: OutboundProxyMode,
    /// Redacted manual proxy for `http://` requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_proxy: Option<String>,
    /// Redacted manual proxy for `https://` requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub https_proxy: Option<String>,
    /// User bypass entries.
    #[serde(default)]
    pub no_proxy: Vec<String>,
    /// The resolved proxy, when the daemon could resolve one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effective: Option<EffectiveProxy>,
}

/// Result of a connection test through the resolved proxy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetworkTestResult {
    /// Whether the request got a successful HTTP status.
    pub ok: bool,
    /// The URL requested.
    pub url: String,
    /// HTTP status, when a response arrived.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Scrubbed error, when the request failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// Redacted proxy the request went through; `None` for a direct request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    /// Which source chose `via`.
    pub source: ProxySource,
}
