//! System-default path opening abstraction.

use std::path::Path;

use crate::PlatformError;

/// Open a file or directory with the host desktop's default application.
pub trait SystemOpener {
    /// Open `path` using the host desktop integration.
    fn open_path(&self, path: &Path) -> Result<(), PlatformError>;
}
