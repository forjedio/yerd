//! `SystemProxySource` - the OS-level proxy settings (macOS System Settings
//! via `SCDynamicStore`, GNOME via `gsettings`).
//!
//! Parsing lives in [`crate::pure::system_proxy`]; the per-OS impls in `os/`
//! only fetch the raw values, exposed as [`crate::ActiveSystemProxySource`].

use yerd_core::network::{ProxySource, SystemProxy};

/// Read the OS proxy settings.
pub trait SystemProxySource {
    /// Which [`ProxySource`] a reading from this source represents.
    fn kind(&self) -> ProxySource;
    /// The current reading, or `None` when this OS has no readable settings
    /// (tool missing, schema absent, unsupported OS).
    fn read(&self) -> Option<SystemProxy>;
}

/// Test fake returning a fixed reading.
#[derive(Debug, Default, Clone)]
pub struct FakeSystemProxySource {
    /// The reading [`SystemProxySource::read`] returns.
    pub reading: Option<SystemProxy>,
    /// The source [`SystemProxySource::kind`] reports.
    pub kind: ProxySource,
}

impl SystemProxySource for FakeSystemProxySource {
    fn kind(&self) -> ProxySource {
        self.kind
    }

    fn read(&self) -> Option<SystemProxy> {
        self.reading.clone()
    }
}
