//! Pure command→request mapping and response→output rendering.
//!
//! Both directions are I/O-free and unit-tested: `to_request` validates
//! arguments client-side (so a bad name/version is a clean usage error before
//! any connect), and `render` turns a [`Response`] into stdout/stderr text and
//! an exit code.

use yerd_core::{PhpVersion, Site, SiteKind};
use yerd_ipc::{Request, Response};

use crate::cli::Command;
use crate::error::ClientError;

/// Map a parsed [`Command`] to the wire [`Request`], validating site names and
/// PHP versions client-side. `Use` maps to [`Request::SetPhp`].
pub fn to_request(cmd: &Command) -> Result<Request, ClientError> {
    Ok(match cmd {
        Command::Ping => Request::Ping,
        Command::Sites => Request::ListSites,
        Command::Park { path } => Request::Park { path: path.clone() },
        Command::Link { name, path } => {
            validate_name(name)?;
            Request::Link {
                name: name.clone(),
                path: path.clone(),
            }
        }
        Command::Unlink { name } => {
            validate_name(name)?;
            Request::Unlink { name: name.clone() }
        }
        Command::Use { name, version } => {
            validate_name(name)?;
            Request::SetPhp {
                name: name.clone(),
                version: parse_php(version)?,
            }
        }
    })
}

fn parse_php(s: &str) -> Result<PhpVersion, ClientError> {
    s.parse::<PhpVersion>()
        .map_err(|e| ClientError::Usage(format!("invalid PHP version {s:?}: {e}")))
}

/// Validate a site name client-side by constructing a throwaway `Site` (the
/// document root is irrelevant — only the name is checked).
fn validate_name(name: &str) -> Result<(), ClientError> {
    Site::linked(name, "/", PhpVersion::new(8, 3))
        .map(|_| ())
        .map_err(|e| ClientError::Usage(format!("invalid site name {name:?}: {e}")))
}

/// The result of rendering a response: text to print and a process exit code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rendered {
    /// Text for stdout (may be empty).
    pub stdout: String,
    /// Text for stderr (may be empty).
    pub stderr: String,
    /// Process exit code.
    pub code: u8,
}

impl Rendered {
    fn ok(stdout: String) -> Self {
        Self {
            stdout,
            stderr: String::new(),
            code: 0,
        }
    }

    fn err(stderr: String) -> Self {
        Self {
            stdout: String::new(),
            stderr,
            code: 1,
        }
    }
}

/// Render a daemon [`Response`] to stdout/stderr text + an exit code. With
/// `json`, prints the response as pretty JSON instead of a human table.
#[must_use]
pub fn render(resp: &Response, json: bool) -> Rendered {
    if json {
        let body = serde_json::to_string_pretty(resp)
            .unwrap_or_else(|e| format!("{{\"error\":\"serialize failed: {e}\"}}"));
        let code = u8::from(matches!(resp, Response::Error { .. }));
        return Rendered {
            stdout: body,
            stderr: String::new(),
            code,
        };
    }
    match resp {
        Response::Pong => Rendered::ok("pong".to_owned()),
        Response::Ok => Rendered::ok("ok".to_owned()),
        Response::Sites { sites } => Rendered::ok(format_sites(sites)),
        Response::Error { code, message } => Rendered::err(format!("error ({code:?}): {message}")),
        // `Response` is `#[non_exhaustive]`; a future variant from a newer
        // daemon is surfaced benignly rather than panicking.
        _ => Rendered::err("unexpected response from daemon".to_owned()),
    }
}

fn format_sites(sites: &[Site]) -> String {
    if sites.is_empty() {
        return "no sites".to_owned();
    }
    let mut out = String::from("NAME\tKIND\tPHP\tSECURE\tDOCROOT");
    for s in sites {
        let kind = match s.kind() {
            SiteKind::Parked => "parked",
            SiteKind::Linked => "linked",
        };
        out.push_str(&format!(
            "\n{}\t{}\t{}\t{}\t{}",
            s.name(),
            kind,
            s.php(),
            s.secure(),
            s.document_root().display()
        ));
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use yerd_ipc::ErrorCode;

    #[test]
    fn maps_each_command_to_its_request() {
        assert_eq!(to_request(&Command::Ping).unwrap(), Request::Ping);
        assert_eq!(to_request(&Command::Sites).unwrap(), Request::ListSites);
        assert_eq!(
            to_request(&Command::Park {
                path: PathBuf::from("/srv/sites")
            })
            .unwrap(),
            Request::Park {
                path: PathBuf::from("/srv/sites")
            }
        );
        assert_eq!(
            to_request(&Command::Link {
                name: "foo".into(),
                path: PathBuf::from("/srv/foo")
            })
            .unwrap(),
            Request::Link {
                name: "foo".into(),
                path: PathBuf::from("/srv/foo")
            }
        );
        assert_eq!(
            to_request(&Command::Unlink { name: "foo".into() }).unwrap(),
            Request::Unlink { name: "foo".into() }
        );
        // `use` maps to SetPhp.
        assert_eq!(
            to_request(&Command::Use {
                name: "foo".into(),
                version: "8.4".into()
            })
            .unwrap(),
            Request::SetPhp {
                name: "foo".into(),
                version: PhpVersion::new(8, 4)
            }
        );
    }

    #[test]
    fn rejects_bad_version_and_name_before_connect() {
        match to_request(&Command::Use {
            name: "foo".into(),
            version: "not-a-version".into(),
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Link {
            name: "bad name".into(),
            path: PathBuf::from("/x"),
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
        match to_request(&Command::Unlink {
            name: "bad/name".into(),
        }) {
            Err(ClientError::Usage(_)) => {}
            other => panic!("expected Usage error, got {other:?}"),
        }
    }

    #[test]
    fn renders_human_responses_and_exit_codes() {
        assert_eq!(render(&Response::Pong, false).stdout, "pong");
        assert_eq!(render(&Response::Pong, false).code, 0);
        assert_eq!(render(&Response::Ok, false).code, 0);

        let empty = render(&Response::Sites { sites: vec![] }, false);
        assert_eq!(empty.stdout, "no sites");
        assert_eq!(empty.code, 0);

        let site = Site::linked("foo", "/srv/foo", PhpVersion::new(8, 3)).unwrap();
        let listed = render(&Response::Sites { sites: vec![site] }, false);
        assert!(listed.stdout.contains("foo"));
        assert!(listed.stdout.contains("linked"));
        assert!(listed.stdout.contains("8.3"));
        assert_eq!(listed.code, 0);

        let err = render(
            &Response::Error {
                code: ErrorCode::NotFound,
                message: "nope".into(),
            },
            false,
        );
        assert!(err.stdout.is_empty());
        assert!(err.stderr.contains("nope"));
        assert_eq!(err.code, 1);
    }

    #[test]
    fn json_rendering_is_valid_and_codes_match() {
        let ok = render(&Response::Ok, true);
        assert!(serde_json::from_str::<serde_json::Value>(&ok.stdout).is_ok());
        assert_eq!(ok.code, 0);

        let err = render(
            &Response::Error {
                code: ErrorCode::Internal,
                message: "boom".into(),
            },
            true,
        );
        let v: serde_json::Value = serde_json::from_str(&err.stdout).unwrap();
        assert_eq!(v["type"], "error");
        assert_eq!(err.code, 1);
    }
}
