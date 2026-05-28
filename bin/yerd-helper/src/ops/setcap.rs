//! `setcap` operation. Linux only.

use std::path::Path;

use crate::error::HelperError;
use crate::ops::run_command;
use crate::validate;

#[cfg(target_os = "linux")]
pub fn setcap(binary: &Path) -> Result<(), HelperError> {
    validate::require_existing_file(binary)?;
    validate::require_basename_yerdd(binary)?;
    let binary_str = binary.to_string_lossy();
    run_command(
        "setcap",
        "setcap",
        ["cap_net_bind_service=+ep", binary_str.as_ref()],
    )
    .map(|_| ())
}

#[cfg(target_os = "macos")]
pub fn setcap(_binary: &Path) -> Result<(), HelperError> {
    Err(HelperError::Unsupported {
        operation: yerd_platform::error::ops::SETCAP,
    })
}
