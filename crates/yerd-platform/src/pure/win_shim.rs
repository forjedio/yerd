//! Pure rendering + recognition of the Windows `.cmd` shim wrappers.
//!
//! On Windows a batch file cannot set `argv[0]`, so the multi-call shims that
//! are symlinks on Unix become `.cmd` wrappers that re-invoke `yerd.exe` with an
//! explicit `__shim <tool>` sentinel (see `bin/yerd`'s shim dispatch). This
//! module owns the exact wrapper body and the ownership probe the daemon's
//! reconcile uses to prune only wrappers Yerd itself wrote, never a user's own
//! `php.cmd`.
//!
//! Un-gated (compiled and table-tested on every OS): the functions are pure
//! string/path formatting with no OS effect.

use std::path::Path;

/// The `.cmd` wrapper body for `shim_name`, re-invoking `yerd_exe` under the
/// `__shim` sentinel and propagating the child's exit code.
///
/// CRLF line endings are deliberate: `cmd.exe` mishandles bare-LF batch files.
#[must_use]
pub fn wrapper_body(yerd_exe: &Path, shim_name: &str) -> String {
    let exe = yerd_exe.display();
    format!("@echo off\r\n\"{exe}\" __shim {shim_name} %*\r\nexit /b %ERRORLEVEL%\r\n")
}

/// The wrapper file name for a shim: `<shim>.cmd`.
#[must_use]
pub fn wrapper_file_name(shim: &str) -> String {
    format!("{shim}.cmd")
}

/// Ownership marker embedded in a forwarding wrapper (see [`forward_wrapper_body`]).
const FORWARD_MARKER: &str = "yerd-forward-shim";

/// The `.cmd` forwarding wrapper body that invokes a real foreign executable
/// `target` (Node's `node.exe`/`npm.cmd`/`npx.cmd`, Bun's `bun.exe`), optionally
/// prefixing `prefix_args` before the caller's arguments and propagating the exit
/// code. Unlike [`wrapper_body`], which re-enters `yerd.exe` under `__shim`, this
/// calls the tool's own binary directly.
///
/// `call` (not a bare invocation) is deliberate: it returns control to the
/// wrapper even when `target` is itself a batch file (`npm.cmd`/`npx.cmd`), so
/// `exit /b %ERRORLEVEL%` still runs and the child's code propagates. CRLF line
/// endings match [`wrapper_body`] (`cmd.exe` mishandles bare-LF batch files).
#[must_use]
pub fn forward_wrapper_body(target: &Path, prefix_args: &[&str]) -> String {
    let exe = target.display();
    let mut lead = String::new();
    for arg in prefix_args {
        lead.push_str(arg);
        lead.push(' ');
    }
    format!(
        "@echo off\r\n@rem {FORWARD_MARKER}\r\ncall \"{exe}\" {lead}%*\r\nexit /b %ERRORLEVEL%\r\n"
    )
}

/// Whether `content` is a Yerd-written forwarding wrapper (its `@rem` marker
/// line). A hand-written `node.cmd` is vanishingly unlikely to carry this exact
/// marker, so pruning gated on it never deletes a file Yerd didn't create. This
/// is distinct from [`is_yerd_wrapper`]: forwarding wrappers carry no `__shim`
/// line, so the two ownership probes never overlap.
#[must_use]
pub fn is_yerd_forward_wrapper(content: &str) -> bool {
    content
        .lines()
        .any(|line| line.trim() == format!("@rem {FORWARD_MARKER}"))
}

/// Whether `content` is a Yerd-written `.cmd` wrapper. The `" __shim "`
/// invocation line is the ownership marker: a hand-written `php.cmd` is
/// vanishingly unlikely to contain a quoted-exe `__shim` dispatch line ending in
/// `%*`, so pruning gated on this never deletes a file Yerd didn't create.
#[must_use]
pub fn is_yerd_wrapper(content: &str) -> bool {
    content.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.contains("\" __shim ") && trimmed.ends_with("%*")
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn wrapper_body_is_exact_crlf_shape() {
        let exe = PathBuf::from(r"C:\Users\me\AppData\Local\Programs\yerd\bin\yerd.exe");
        let body = wrapper_body(&exe, "php");
        assert_eq!(
            body,
            "@echo off\r\n\"C:\\Users\\me\\AppData\\Local\\Programs\\yerd\\bin\\yerd.exe\" __shim php %*\r\nexit /b %ERRORLEVEL%\r\n"
        );
    }

    #[test]
    fn wrapper_body_handles_spaces_in_exe_path() {
        let exe = PathBuf::from(r"C:\Program Files\Yerd App\yerd.exe");
        let body = wrapper_body(&exe, "php8.4cover");
        assert!(body.contains("\"C:\\Program Files\\Yerd App\\yerd.exe\" __shim php8.4cover %*"));
        assert!(is_yerd_wrapper(&body));
    }

    #[test]
    fn wrapper_file_name_appends_cmd() {
        assert_eq!(wrapper_file_name("php"), "php.cmd");
        assert_eq!(wrapper_file_name("php8.4cover"), "php8.4cover.cmd");
        assert_eq!(wrapper_file_name("composer"), "composer.cmd");
    }

    #[test]
    fn is_yerd_wrapper_recognises_own_output() {
        for shim in [
            "php",
            "php8.4",
            "php8.4cover",
            "phpcover",
            "composer",
            "wp",
            "laravel",
        ] {
            let body = wrapper_body(Path::new(r"C:\bin\yerd.exe"), shim);
            assert!(is_yerd_wrapper(&body), "shim {shim}");
        }
    }

    #[test]
    fn is_yerd_wrapper_rejects_foreign_content() {
        assert!(!is_yerd_wrapper("@echo off\r\nphp.exe %*\r\n"));
        assert!(!is_yerd_wrapper("@echo off\r\n\"C:\\php\\php.exe\" %*\r\n"));
        assert!(!is_yerd_wrapper(""));
        assert!(!is_yerd_wrapper("__shim php"));
        assert!(!is_yerd_wrapper("echo __shim is a great tool"));
    }

    #[test]
    fn forward_wrapper_body_is_exact_crlf_shape() {
        let target = PathBuf::from(
            r"C:\Users\me\AppData\Local\yerd\tools\node\node-v24.17.0-win-x64\node.exe",
        );
        let body = forward_wrapper_body(&target, &[]);
        assert_eq!(
            body,
            "@echo off\r\n@rem yerd-forward-shim\r\ncall \"C:\\Users\\me\\AppData\\Local\\yerd\\tools\\node\\node-v24.17.0-win-x64\\node.exe\" %*\r\nexit /b %ERRORLEVEL%\r\n"
        );
        assert!(is_yerd_forward_wrapper(&body));
    }

    #[test]
    fn forward_wrapper_body_prefixes_args() {
        let target = PathBuf::from(r"C:\bin\bun.exe");
        let body = forward_wrapper_body(&target, &["x"]);
        assert!(body.contains("call \"C:\\bin\\bun.exe\" x %*"));
        assert!(is_yerd_forward_wrapper(&body));
    }

    #[test]
    fn forward_wrapper_handles_spaces_in_target_path() {
        let target = PathBuf::from(r"C:\Program Files\yerd\tools\node\npm.cmd");
        let body = forward_wrapper_body(&target, &[]);
        assert!(body.contains("call \"C:\\Program Files\\yerd\\tools\\node\\npm.cmd\" %*"));
    }

    #[test]
    fn forward_and_multicall_ownership_probes_dont_overlap() {
        let multicall = wrapper_body(Path::new(r"C:\bin\yerd.exe"), "composer");
        let forward = forward_wrapper_body(Path::new(r"C:\bin\node.exe"), &[]);
        assert!(is_yerd_wrapper(&multicall) && !is_yerd_forward_wrapper(&multicall));
        assert!(is_yerd_forward_wrapper(&forward) && !is_yerd_wrapper(&forward));
    }

    #[test]
    fn is_yerd_forward_wrapper_rejects_foreign_content() {
        assert!(!is_yerd_forward_wrapper(
            "@echo off\r\ncall \"C:\\node\\node.exe\" %*\r\n"
        ));
        assert!(!is_yerd_forward_wrapper(""));
        assert!(!is_yerd_forward_wrapper("@rem yerd-forward-shim is neat"));
    }
}
