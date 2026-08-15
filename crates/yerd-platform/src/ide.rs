//! Host IDE detection and project launching abstraction.
//!
//! Detection resolves a launch target once; launching consumes that target and
//! never re-scans the host. The IDE identity itself is the `id` column of
//! [`crate::pure::ide_spec::IDE_SPECS`], so adding an editor is a one-row change.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::IdeErrorReason;
use crate::PlatformError;

/// How a detected IDE is started.
///
/// `Application` is the host's indirect launch handle: a `.desktop` entry path
/// on Linux, a `.app` bundle path on macOS. Keeping it OS-neutral lets every
/// adapter implement exactly two arms without a `cfg`-gated variant.
#[derive(Clone, Debug, PartialEq)]
pub enum LaunchTarget {
    /// A command-line launcher invoked directly with the project path.
    Cli(PathBuf),
    /// A desktop-integration handle opened through the host's launcher.
    Application(PathBuf),
}

/// One IDE found on this host, with the target used to launch it.
#[derive(Clone, Debug, PartialEq)]
pub struct DetectedIde {
    /// Stable IDE identifier from `IDE_SPECS`.
    pub id: &'static str,
    /// User-facing name from `IDE_SPECS`.
    pub display_name: &'static str,
    /// Resolved launch target.
    pub launch: LaunchTarget,
}

/// Detect installed IDEs and open project directories in one of them.
pub trait IdeLauncher {
    /// Return the IDEs available on this host, best first (lowest `rank`).
    fn detect(&self) -> Vec<DetectedIde>;

    /// Open `path` in a previously detected IDE.
    fn launch(&self, ide: &DetectedIde, path: &Path) -> Result<(), PlatformError>;
}

/// Test fake returning a fixed detection list and recording every launch.
#[derive(Debug, Default)]
pub struct FakeIdeLauncher {
    detected: Vec<DetectedIde>,
    launch_error: Option<std::io::ErrorKind>,
    launches: Mutex<Vec<(String, PathBuf)>>,
}

impl FakeIdeLauncher {
    /// A fake that reports `detected` and launches successfully.
    #[must_use]
    pub fn new(detected: Vec<DetectedIde>) -> Self {
        Self {
            detected,
            launch_error: None,
            launches: Mutex::new(Vec::new()),
        }
    }

    /// A fake that reports `detected` but fails every launch with `kind`.
    #[must_use]
    pub fn failing(detected: Vec<DetectedIde>, kind: std::io::ErrorKind) -> Self {
        Self {
            detected,
            launch_error: Some(kind),
            launches: Mutex::new(Vec::new()),
        }
    }

    /// Every `(ide id, path)` pair passed to [`IdeLauncher::launch`], in order.
    #[must_use]
    pub fn launches(&self) -> Vec<(String, PathBuf)> {
        self.guard().clone()
    }

    /// A poisoned lock still holds the recorded calls, so recover rather than
    /// propagate: this is a test double, not a correctness boundary.
    fn guard(&self) -> MutexGuard<'_, Vec<(String, PathBuf)>> {
        self.launches.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl IdeLauncher for FakeIdeLauncher {
    fn detect(&self) -> Vec<DetectedIde> {
        self.detected.clone()
    }

    fn launch(&self, ide: &DetectedIde, path: &Path) -> Result<(), PlatformError> {
        self.guard().push((ide.id.to_owned(), path.to_path_buf()));
        match self.launch_error {
            None => Ok(()),
            Some(kind) => Err(PlatformError::Ide {
                reason: IdeErrorReason::Launch {
                    ide: ide.display_name.to_owned(),
                    source: std::io::Error::from(kind),
                },
            }),
        }
    }
}
