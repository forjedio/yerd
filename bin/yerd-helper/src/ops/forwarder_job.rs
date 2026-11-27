//! Install and remove the launchd port-forwarder jobs. macOS only.
//!
//! Each scope gets two root-owned launchd daemons (one per privileged port) whose
//! `Sockets` make launchd bind 80/443 and hand the listening socket to
//! `yerd-forward`, run as the invoking user. See
//! `yerd_platform::pure::launchd_ports` for the plist shape and the safety
//! argument. Both install and uninstall also remove any legacy pf redirect
//! (see [`super::legacy_pf`]); install does so only once the new jobs have
//! loaded, so a failed install never leaves the user with no redirect at all.

#![allow(clippy::similar_names)]

use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use yerd_platform::pure::launchd_ports::{self, Job, Role, Scope};

use crate::error::{HelperError, ValidationReason};
use crate::ops::{
    atomic_write, bootout_system, bootstrap_system, invalid, legacy_pf, remove_if_present,
    resolve_user, run_command,
};

const PRIVILEGED_CEILING: u16 = 1024;

/// Validated install parameters for one scope.
#[derive(Debug)]
struct Plan {
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
}

/// Install the scope's two forwarder jobs, replacing any existing ones.
///
/// Validates everything before touching the system. If any job can't be
/// written or loaded, both of the scope's jobs are removed again so no
/// half-installed scope is left, and any legacy pf redirect stays in place.
/// Legacy pf state is removed only after both jobs have loaded.
pub fn install(
    scope: Scope,
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
    uid: u32,
) -> Result<(), HelperError> {
    let plan = validate_ports(http_from, http_to, https_from, https_to)?;
    let user = resolve_user(uid)?.name;
    let forwarder = locate_forwarder()?;

    let jobs = scope.jobs();
    let result = jobs
        .iter()
        .try_for_each(|job| install_job(scope, job, &forwarder, &user, &plan));
    if result.is_err() {
        for job in &jobs {
            remove_job(job);
        }
        return result;
    }
    legacy_pf::remove_all_legacy()
}

/// Remove the scope's forwarder jobs (absent is fine), then any legacy pf
/// redirect.
pub fn uninstall(scope: Scope) -> Result<(), HelperError> {
    for job in &scope.jobs() {
        bootout_system(job.label);
        remove_if_present(job.plist)?;
    }
    legacy_pf::remove_all_legacy()
}

fn validate_ports(
    http_from: u16,
    http_to: u16,
    https_from: u16,
    https_to: u16,
) -> Result<Plan, HelperError> {
    for (port, flag) in [
        (http_from, "--http-from"),
        (http_to, "--http-to"),
        (https_from, "--https-from"),
        (https_to, "--https-to"),
    ] {
        if port == 0 {
            return Err(invalid(ValidationReason::PortInvalid(flag)));
        }
    }
    if http_from >= PRIVILEGED_CEILING {
        return Err(invalid(ValidationReason::ForwardSourceInvalid(
            "--http-from",
        )));
    }
    if https_from >= PRIVILEGED_CEILING || https_from == http_from {
        return Err(invalid(ValidationReason::ForwardSourceInvalid(
            "--https-from",
        )));
    }
    if http_to < PRIVILEGED_CEILING || http_to == http_from || http_to == https_from {
        return Err(invalid(ValidationReason::ForwardTargetInvalid("--http-to")));
    }
    if https_to < PRIVILEGED_CEILING
        || https_to == http_from
        || https_to == https_from
        || https_to == http_to
    {
        return Err(invalid(ValidationReason::ForwardTargetInvalid(
            "--https-to",
        )));
    }
    Ok(Plan {
        http_from,
        http_to,
        https_from,
        https_to,
    })
}

/// `yerd-forward` next to this helper: a regular file that only its owner can
/// write, on a path that survives reboot.
fn locate_forwarder() -> Result<PathBuf, HelperError> {
    let exe = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .map_err(|source| HelperError::Io {
            path: PathBuf::from("current_exe"),
            source,
        })?;
    let forwarder = exe
        .parent()
        .map(|dir| dir.join(launchd_ports::FORWARDER_BIN))
        .ok_or_else(|| invalid(ValidationReason::ForwarderInvalid(exe.clone())))?;
    let acceptable = std::fs::metadata(&forwarder)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o022 == 0)
        && launchd_ports::is_persistent_forwarder_path(&forwarder);
    if acceptable {
        Ok(forwarder)
    } else {
        Err(invalid(ValidationReason::ForwarderInvalid(forwarder)))
    }
}

fn install_job(
    scope: Scope,
    job: &Job,
    forwarder: &Path,
    user: &str,
    plan: &Plan,
) -> Result<(), HelperError> {
    let (from, to) = match job.role {
        Role::Http => (plan.http_from, plan.http_to),
        Role::Https => (plan.https_from, plan.https_to),
    };
    let plist = launchd_ports::compose_forwarder_plist(scope, job.label, forwarder, user, from, to);
    atomic_write(Path::new(job.plist), plist.as_bytes(), true)?;
    run_command("chown", "/usr/sbin/chown", ["root:wheel", job.plist])?;
    bootout_system(job.label);
    bootstrap_system(job.plist)
}

fn remove_job(job: &Job) {
    bootout_system(job.label);
    let _ = remove_if_present(job.plist);
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[derive(Clone, Copy)]
    enum Expect {
        Port,
        Source,
        Target,
    }

    #[test]
    fn rejects_bad_ports_before_any_io() {
        let cases = [
            ([0, 8080, 443, 8443], Expect::Port),
            ([80, 0, 443, 8443], Expect::Port),
            ([80, 8080, 443, 1023], Expect::Target),
            ([80, 80, 443, 8443], Expect::Target),
            ([80, 443, 443, 8443], Expect::Target),
            ([80, 8080, 443, 443], Expect::Target),
            ([80, 8080, 443, 8080], Expect::Target),
            ([443, 8080, 443, 8443], Expect::Source),
            ([8000, 8080, 443, 8443], Expect::Source),
            ([80, 8080, 1443, 8443], Expect::Source),
        ];
        for ([hf, ht, sf, st], expect) in cases {
            let err = install(Scope::Loopback, hf, ht, sf, st, 501).unwrap_err();
            let matched = matches!(
                (expect, &err),
                (
                    Expect::Port,
                    HelperError::Validation {
                        reason: ValidationReason::PortInvalid(_),
                    },
                ) | (
                    Expect::Source,
                    HelperError::Validation {
                        reason: ValidationReason::ForwardSourceInvalid(_),
                    },
                ) | (
                    Expect::Target,
                    HelperError::Validation {
                        reason: ValidationReason::ForwardTargetInvalid(_),
                    },
                )
            );
            assert!(matched, "{hf}->{ht}, {sf}->{st}: got {err:?}");
        }
    }

    #[test]
    fn rejects_root_uid_before_any_io() {
        for scope in [Scope::Loopback, Scope::Lan] {
            let err = install(scope, 80, 8080, 443, 8443, 0).unwrap_err();
            assert!(matches!(
                err,
                HelperError::Validation {
                    reason: ValidationReason::UidInvalid
                }
            ));
        }
    }
}
