//! Dispatcher: typed `HelperInvocation` → per-op implementation.

use yerd_platform::HelperInvocation;

use crate::error::HelperError;
use crate::ops;

/// Run the operation and return any error.
pub fn dispatch(inv: HelperInvocation) -> Result<(), HelperError> {
    match inv {
        HelperInvocation::InstallCa { ca_pem_path, fp } => ops::ca::install_ca(&ca_pem_path, &fp),
        HelperInvocation::UninstallCa { fp } => ops::ca::uninstall_ca(&fp),
        HelperInvocation::InstallResolver { tld, addr } => {
            ops::resolver::install_resolver(&tld, addr)
        }
        HelperInvocation::UninstallResolver { tld } => ops::resolver::uninstall_resolver(&tld),
        HelperInvocation::Setcap { daemon_binary } => ops::setcap::setcap(&daemon_binary),
        HelperInvocation::InstallPortRedirect {
            http_from,
            http_to,
            https_from,
            https_to,
        } => ops::port_redirect::install_port_redirect(http_from, http_to, https_from, https_to),
        HelperInvocation::UninstallPortRedirect => ops::port_redirect::uninstall_port_redirect(),
        _ => Err(HelperError::Unsupported {
            operation: "unknown-variant",
        }),
    }
}
