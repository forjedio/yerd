//! `LibvipsProbe` - is libvips installed on this host?
//!
//! Only used to show an advisory hint next to the GUI's FFI toggle: php-vips
//! needs libvips, which Yerd does not bundle. Checking is a handful of `stat`
//! calls over [`crate::pure::libvips`]'s candidate lists, done by the per-OS
//! impls in `os/` and exposed as [`crate::ActiveLibvipsProbe`].

/// Detect whether the libvips shared library is installed.
pub trait LibvipsProbe {
    /// `Some(true)` when libvips was found, `Some(false)` when it was looked for
    /// and not found, `None` when this OS is not checked.
    fn libvips_found(&self) -> Option<bool>;
}

/// Test fake returning a fixed answer.
#[derive(Debug, Default, Clone, Copy)]
pub struct FakeLibvipsProbe {
    /// The answer [`LibvipsProbe::libvips_found`] returns.
    pub found: Option<bool>,
}

impl LibvipsProbe for FakeLibvipsProbe {
    fn libvips_found(&self) -> Option<bool> {
        self.found
    }
}
