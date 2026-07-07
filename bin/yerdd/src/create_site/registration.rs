//! Register a freshly scaffolded project so the proxy serves it.
//!
//! Framework-agnostic: parks or links the project directory, then applies
//! `SetPhp`/`SetSecure` if the spec asked for something other than the
//! config's defaults. Shared by every framework's create-site job - it's
//! just a client of the ordinary mutation API (`handle_mutation`), the same
//! one `Request::Park`/`Link`/`SetPhp`/`SetSecure` go through directly.

use std::path::Path;
use std::sync::Arc;

use yerd_ipc::{CreateSiteSpec, Request, Response};

use crate::state::DaemonState;

/// Park-or-link `project_dir` under `name`, then apply `SetPhp`/`SetSecure` if
/// the spec differs from the config's current defaults.
pub(super) async fn register(
    name: &str,
    parent_dir: &Path,
    project_dir: &Path,
    spec: &CreateSiteSpec,
    state: &Arc<DaemonState>,
) -> Result<(), String> {
    let parent_canon = tokio::fs::canonicalize(parent_dir)
        .await
        .unwrap_or_else(|_| parent_dir.to_path_buf());
    let (is_parked, default_php) = {
        let cfg = state.config.lock().await;
        let parked = cfg
            .parked
            .paths
            .contains(parent_canon.to_string_lossy().as_ref());
        (parked, cfg.php.default)
    };

    if is_parked {
        mutate_ok(
            crate::ipc_server::handle_mutation(
                Request::Park {
                    path: parent_dir.to_path_buf(),
                },
                state,
            )
            .await,
        )?;
    } else {
        mutate_ok(
            crate::ipc_server::handle_mutation(
                Request::Link {
                    name: name.to_owned(),
                    path: project_dir.to_path_buf(),
                },
                state,
            )
            .await,
        )?;
    }

    if spec.php != default_php {
        mutate_ok(
            crate::ipc_server::handle_mutation(
                Request::SetPhp {
                    name: name.to_owned(),
                    version: spec.php,
                },
                state,
            )
            .await,
        )?;
    }
    if spec.secure {
        mutate_ok(
            crate::ipc_server::handle_mutation(
                Request::SetSecure {
                    name: name.to_owned(),
                    secure: true,
                },
                state,
            )
            .await,
        )?;
    }
    Ok(())
}

/// Map a mutation `Response` to `Result`.
pub(super) fn mutate_ok(resp: Response) -> Result<(), String> {
    match resp {
        Response::Ok => Ok(()),
        Response::Error { message, .. } => Err(message),
        other => Err(format!("unexpected response: {other:?}")),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use yerd_ipc::ErrorCode;

    #[test]
    fn mutate_ok_maps_responses() {
        assert!(mutate_ok(Response::Ok).is_ok());
        assert_eq!(
            mutate_ok(Response::Error {
                code: ErrorCode::Internal,
                message: "boom".to_owned(),
            }),
            Err("boom".to_owned())
        );
        assert!(mutate_ok(Response::JobStarted {
            job_id: "j1".to_owned()
        })
        .is_err());
    }
}
