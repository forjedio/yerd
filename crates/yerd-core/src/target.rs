//! Host target detection: which operating system and CPU architecture Yerd is
//! running on, in the vocabulary its prebuilt artifact filenames use.
//!
//! Every managed download names its artifact with these tokens, so PHP builds,
//! service engines, Node, Bun and cloudflared all resolve through the same two
//! enums rather than each crate carrying its own copy.

/// Target operating system for a prebuilt artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    /// Linux (glibc build - can load shared extensions; the manifest never
    /// ships a fully-static musl build, which can't `dlopen`).
    Linux,
    /// macOS.
    Macos,
    /// Windows (repackaged `windows.php.net` bundle: `php.exe` + `php-cgi.exe`,
    /// `x86_64` only).
    Windows,
}

impl Os {
    /// The token used in artifact filenames.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        }
    }
}

/// Target CPU architecture for a prebuilt artifact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    /// 64-bit x86.
    X86_64,
    /// 64-bit ARM.
    Aarch64,
}

impl Arch {
    /// The token used in artifact filenames.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Arch::X86_64 => "x86_64",
            Arch::Aarch64 => "aarch64",
        }
    }
}

/// Which half of the host target Yerd ships no prebuilt artifact for.
///
/// Deliberately not an error type: this crate is dependency-free and each
/// calling crate renders the failure in its own error vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnsupportedTarget {
    /// `std::env::consts::OS` is not one Yerd builds for.
    Os(&'static str),
    /// `std::env::consts::ARCH` is not one Yerd builds for.
    Arch(&'static str),
}

/// Detect the running platform, erroring on anything Yerd has no prebuilt
/// artifact for (e.g. a 32-bit or unknown OS). Windows resolves to
/// [`Os::Windows`] (`x86_64` only); a `windows-aarch64` host resolves fine here
/// but later fails artifact resolution, which is accurate. Call this **before**
/// any download.
///
/// Pure: `std::env::consts` are compile-time constants baked in by the compiler,
/// not an environment read, so this stays inside the crate's no-I/O rule.
pub fn current_os_arch() -> Result<(Os, Arch), UnsupportedTarget> {
    let os = match std::env::consts::OS {
        "linux" => Os::Linux,
        "macos" => Os::Macos,
        "windows" => Os::Windows,
        other => return Err(UnsupportedTarget::Os(other)),
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => Arch::X86_64,
        "aarch64" => Arch::Aarch64,
        other => return Err(UnsupportedTarget::Arch(other)),
    };
    Ok((os, arch))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn os_tokens_match_artifact_filenames() {
        for (os, token) in [
            (Os::Linux, "linux"),
            (Os::Macos, "macos"),
            (Os::Windows, "windows"),
        ] {
            assert_eq!(os.as_str(), token);
        }
    }

    #[test]
    fn arch_tokens_match_artifact_filenames() {
        for (arch, token) in [(Arch::X86_64, "x86_64"), (Arch::Aarch64, "aarch64")] {
            assert_eq!(arch.as_str(), token);
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn current_os_arch_is_linux_x86_64() {
        assert_eq!(current_os_arch().unwrap(), (Os::Linux, Arch::X86_64));
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn current_os_arch_is_macos_aarch64() {
        assert_eq!(current_os_arch().unwrap(), (Os::Macos, Arch::Aarch64));
    }

    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    #[test]
    fn current_os_arch_is_windows_x86_64() {
        assert_eq!(current_os_arch().unwrap(), (Os::Windows, Arch::X86_64));
    }
}
