//! System-default path opening abstraction.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::error::OpenErrorReason;
use crate::PlatformError;

/// Open a file or directory with the host desktop's default application.
pub trait SystemOpener {
    /// Open `path` using the host desktop integration.
    fn open_path(&self, path: &Path) -> Result<(), PlatformError>;
}

/// Test fake recording every opened path.
#[derive(Debug, Default)]
pub struct FakeSystemOpener {
    open_error: Option<std::io::ErrorKind>,
    opened: Mutex<Vec<PathBuf>>,
}

impl FakeSystemOpener {
    /// A fake that opens successfully.
    #[must_use]
    pub fn new() -> Self {
        Self {
            open_error: None,
            opened: Mutex::new(Vec::new()),
        }
    }

    /// A fake whose every open fails with `kind`.
    #[must_use]
    pub fn failing(kind: std::io::ErrorKind) -> Self {
        Self {
            open_error: Some(kind),
            opened: Mutex::new(Vec::new()),
        }
    }

    /// Every path passed to [`SystemOpener::open_path`], in order.
    #[must_use]
    pub fn opened(&self) -> Vec<PathBuf> {
        self.guard().clone()
    }

    /// A poisoned lock still holds the recorded calls, so recover rather than
    /// propagate: this is a test double, not a correctness boundary.
    fn guard(&self) -> MutexGuard<'_, Vec<PathBuf>> {
        self.opened.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl SystemOpener for FakeSystemOpener {
    fn open_path(&self, path: &Path) -> Result<(), PlatformError> {
        self.guard().push(path.to_path_buf());
        match self.open_error {
            None => Ok(()),
            Some(kind) => Err(PlatformError::SystemOpen {
                reason: OpenErrorReason::Launch {
                    program: "fake-opener".to_owned(),
                    source: std::io::Error::from(kind),
                },
            }),
        }
    }
}
