//! Compose and edit the yerd-owned PATH block in a shell rc file.
//!
//! Every function here is pure: string→string, no I/O, no env, no clock. The
//! binary edge (`yerd path`) reads `$SHELL`/`$HOME`, picks the rc file(s), reads
//! their contents, calls [`upsert_block`]/[`remove_block`], and writes back.
//!
//! The block adds `{data}/bin` (where yerd keeps its `php`/`composer` shims) to
//! `PATH`, **prepended** so it wins over other managers (e.g. Herd) and
//! **guarded** so re-sourcing an rc file never appends a duplicate entry.

use std::path::{Path, PathBuf};

/// The shells we know how to edit. `Posix` is the generic `.profile` fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    /// Z shell - `~/.zshrc`.
    Zsh,
    /// Bash - `~/.bashrc` + `~/.bash_profile` (see [`rc_relpaths`]).
    Bash,
    /// Fish - `~/.config/fish/config.fish`.
    Fish,
    /// POSIX `sh` and unknown shells - `~/.profile`.
    Posix,
}

/// Host OS, passed in by the caller so this stays env-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    /// macOS - login shells (Terminal.app) read `.bash_profile`.
    MacOs,
    /// Linux and other Unix.
    Linux,
}

/// Opening marker line of the managed block.
const MARKER_OPEN: &str = "# >>> yerd PATH >>>";
/// Closing marker line of the managed block.
const MARKER_CLOSE: &str = "# <<< yerd PATH <<<";

/// Map a `$SHELL` basename (e.g. `"zsh"`, `"bash"`, `"fish"`) to a [`Shell`].
/// Unknown names → `None` so the caller can refuse rather than guess. A bare
/// `"sh"` maps to [`Shell::Posix`].
#[must_use]
pub fn detect_shell(shell_env_basename: &str) -> Option<Shell> {
    match shell_env_basename {
        "zsh" => Some(Shell::Zsh),
        "bash" => Some(Shell::Bash),
        "fish" => Some(Shell::Fish),
        "sh" | "dash" => Some(Shell::Posix),
        _ => None,
    }
}

/// rc file path(s) for `shell`, relative to `$HOME`. Bash returns **two**
/// (`.bashrc` for non-login/interactive shells incl. iTerm & Linux, plus
/// `.bash_profile` which macOS Terminal.app reads as a login shell); the
/// re-source guard makes loading both harmless.
#[must_use]
pub fn rc_relpaths(shell: Shell, _os: HostOs) -> Vec<PathBuf> {
    match shell {
        Shell::Zsh => vec![PathBuf::from(".zshrc")],
        Shell::Bash => vec![PathBuf::from(".bashrc"), PathBuf::from(".bash_profile")],
        Shell::Fish => vec![[".config", "fish", "config.fish"].iter().collect()],
        Shell::Posix => vec![PathBuf::from(".profile")],
    }
}

/// Escape a value for a **POSIX** double-quoted string: `\`, `$`, `` ` ``, and
/// `"` are special inside `"…"`, so backslash-escape them. (Quoting alone only
/// handles spaces; a data dir containing `$`/`` ` ``/`\` would otherwise expand
/// or break the rc block - e.g. a home under `$XDG_DATA_HOME`.)
fn esc_posix_dq(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '$' | '`' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Escape a value for a **fish** double-quoted string: only `\`, `$`, and `"`
/// are special (fish has no backtick command substitution).
fn esc_fish_dq(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if matches!(c, '\\' | '$' | '"') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// The shell body (no markers) that prepends `bin_dir` to `PATH` (guarded so a
/// repeated `source` never stacks a duplicate entry) and exports `PHPRC` to
/// Yerd's generated CLI ini (`{data}/php-cli.ini`, a sibling of `bin_dir`) so the
/// `php` shim on this PATH picks up Yerd's opinionated CLI defaults (memory limit
/// etc.). Values are double-quoted *and* metacharacter-escaped, so a data dir
/// containing a space, `$`, `` ` ``, `\`, or `"` is emitted safely. An absent ini
/// file is harmless - PHP ignores it.
#[must_use]
pub fn render_body(shell: Shell, bin_dir: &Path) -> String {
    let raw_dir = bin_dir.display().to_string();
    let raw_phprc = bin_dir
        .parent()
        .map(|d| d.join("php-cli.ini").display().to_string());
    match shell {
        Shell::Fish => {
            let dir = esc_fish_dq(&raw_dir);
            let mut s =
                format!("if not contains \"{dir}\" $PATH\n    set -gx PATH \"{dir}\" $PATH\nend");
            if let Some(ini) = raw_phprc {
                s.push_str("\nset -gx PHPRC \"");
                s.push_str(&esc_fish_dq(&ini));
                s.push('"');
            }
            s
        }
        Shell::Zsh | Shell::Bash | Shell::Posix => {
            let dir = esc_posix_dq(&raw_dir);
            let mut s = format!(
                "case \":$PATH:\" in\n  *\":{dir}:\"*) ;;\n  *) export PATH=\"{dir}:$PATH\" ;;\nesac"
            );
            if let Some(ini) = raw_phprc {
                s.push_str("\nexport PHPRC=\"");
                s.push_str(&esc_posix_dq(&ini));
                s.push('"');
            }
            s
        }
    }
}

/// The full managed block (markers + body), with a trailing newline.
#[must_use]
pub fn render_block(shell: Shell, bin_dir: &Path) -> String {
    format!(
        "{MARKER_OPEN}\n{}\n{MARKER_CLOSE}\n",
        render_body(shell, bin_dir)
    )
}

/// True if `existing` already contains a yerd-managed block (both markers).
#[must_use]
pub fn contains_block(existing: &str) -> bool {
    marker_line_indices(existing).is_some()
}

/// Insert or replace the managed block in `existing`, returning the new file
/// contents. If the markers are present, the inter-marker region is replaced in
/// place (preserving the block's position); otherwise the block is appended with
/// exactly one blank separator line and a trailing newline. Idempotent: calling
/// twice with the same inputs yields identical output.
#[must_use]
pub fn upsert_block(existing: &str, shell: Shell, bin_dir: &Path) -> String {
    let block = render_block(shell, bin_dir);
    if let Some((open, close)) = marker_line_indices(existing) {
        let lines: Vec<&str> = existing.split('\n').collect();
        let before = join_lines(&lines, 0, open);
        let after = join_lines(&lines, close + 1, lines.len());
        let mut out = String::new();
        if !before.is_empty() {
            out.push_str(&before);
            out.push('\n');
        }
        out.push_str(&block);
        if !after.is_empty() {
            out.push_str(&after);
        }
        out
    } else {
        let mut out = String::from(existing);
        if !out.is_empty() {
            while out.ends_with('\n') {
                out.pop();
            }
            out.push_str("\n\n");
        }
        out.push_str(&block);
        out
    }
}

/// Join `lines[start..end]` with `'\n'`, panic-free (out-of-range bounds are
/// clamped by `take`/`skip`).
fn join_lines(lines: &[&str], start: usize, end: usize) -> String {
    lines
        .iter()
        .take(end)
        .skip(start)
        .copied()
        .collect::<Vec<_>>()
        .join("\n")
}

/// Remove the managed block from `existing`. Deletes the marker pair and the
/// body between them, plus **one immediately-preceding blank line if present**
/// (the separator `upsert_block` inserts). Never consumes a non-empty user line.
/// Returns `existing` unchanged when no block is present.
#[must_use]
pub fn remove_block(existing: &str) -> String {
    let Some((open, close)) = marker_line_indices(existing) else {
        return existing.to_owned();
    };
    let lines: Vec<&str> = existing.split('\n').collect();
    let start = if open > 0 && lines.get(open - 1).is_some_and(|l| l.is_empty()) {
        open - 1
    } else {
        open
    };
    let before = join_lines(&lines, 0, start);
    let after = join_lines(&lines, close + 1, lines.len());
    match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => after,
        (false, true) => {
            let mut s = before;
            if existing.ends_with('\n') {
                s.push('\n');
            }
            s
        }
        (false, false) => format!("{before}\n{after}"),
    }
}

/// Locate the `(open, close)` line indices of the managed block, matching the
/// marker lines exactly (after trimming trailing whitespace) so a user line that
/// merely mentions "yerd" is never treated as a marker. Returns `None` if either
/// marker is missing or they're out of order.
fn marker_line_indices(text: &str) -> Option<(usize, usize)> {
    let mut open = None;
    for (i, line) in text.split('\n').enumerate() {
        let t = line.trim_end();
        if t == MARKER_OPEN && open.is_none() {
            open = Some(i);
        } else if t == MARKER_CLOSE {
            if let Some(o) = open {
                if i > o {
                    return Some((o, i));
                }
            }
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic, clippy::indexing_slicing)]
mod tests {
    use super::*;

    fn bin() -> PathBuf {
        PathBuf::from("/Users/x/Library/Application Support/io.yerd.Yerd/bin")
    }

    #[test]
    fn detect_shell_table() {
        assert_eq!(detect_shell("zsh"), Some(Shell::Zsh));
        assert_eq!(detect_shell("bash"), Some(Shell::Bash));
        assert_eq!(detect_shell("fish"), Some(Shell::Fish));
        assert_eq!(detect_shell("sh"), Some(Shell::Posix));
        assert_eq!(detect_shell("dash"), Some(Shell::Posix));
        assert_eq!(detect_shell("nu"), None);
        assert_eq!(detect_shell(""), None);
    }

    #[test]
    fn rc_relpaths_per_shell() {
        assert_eq!(
            rc_relpaths(Shell::Zsh, HostOs::MacOs),
            vec![PathBuf::from(".zshrc")]
        );
        assert_eq!(
            rc_relpaths(Shell::Bash, HostOs::MacOs),
            vec![PathBuf::from(".bashrc"), PathBuf::from(".bash_profile")]
        );
        let fish = rc_relpaths(Shell::Fish, HostOs::Linux);
        assert!(fish[0].ends_with("config.fish"));
    }

    #[test]
    fn body_is_guarded_and_quotes_the_space() {
        let posix = render_body(Shell::Zsh, &bin());
        assert!(posix.contains("Application Support"));
        assert!(posix.contains(
            "export PATH=\"/Users/x/Library/Application Support/io.yerd.Yerd/bin:$PATH\""
        ));
        assert!(posix.contains("case \":$PATH:\""));
        assert!(posix.contains(") ;;"));

        assert!(posix.contains(
            "export PHPRC=\"/Users/x/Library/Application Support/io.yerd.Yerd/php-cli.ini\""
        ));

        let fish = render_body(Shell::Fish, &bin());
        assert!(fish.contains(
            "if not contains \"/Users/x/Library/Application Support/io.yerd.Yerd/bin\" $PATH"
        ));
        assert!(fish.contains("set -gx PATH"));
        assert!(fish.contains(
            "set -gx PHPRC \"/Users/x/Library/Application Support/io.yerd.Yerd/php-cli.ini\""
        ));
    }

    #[test]
    fn body_escapes_shell_metacharacters_in_the_path() {
        let dir = PathBuf::from(r#"/home/b$x/a`b/c\d/e"f/bin"#);

        let posix = render_body(Shell::Zsh, &dir);
        assert!(posix.contains(r#"export PATH="/home/b\$x/a\`b/c\\d/e\"f/bin:$PATH""#));
        assert!(posix.contains(r#"export PHPRC="/home/b\$x/a\`b/c\\d/e\"f/php-cli.ini""#));

        let fish = render_body(Shell::Fish, &dir);
        assert!(fish.contains(r#"set -gx PATH "/home/b\$x/a`b/c\\d/e\"f/bin" $PATH"#));
        assert!(fish.contains(r#"set -gx PHPRC "/home/b\$x/a`b/c\\d/e\"f/php-cli.ini""#));
    }

    #[test]
    fn upsert_into_empty_file() {
        let out = upsert_block("", Shell::Zsh, &bin());
        assert!(out.starts_with(MARKER_OPEN));
        assert!(out.trim_end().ends_with(MARKER_CLOSE));
        assert!(contains_block(&out));
    }

    #[test]
    fn upsert_is_idempotent() {
        let once = upsert_block("# my zshrc\nexport FOO=1\n", Shell::Zsh, &bin());
        let twice = upsert_block(&once, Shell::Zsh, &bin());
        assert_eq!(once, twice);
        assert_eq!(once.matches(MARKER_OPEN).count(), 1);
    }

    #[test]
    fn upsert_appends_with_single_blank_separator_no_trailing_newline() {
        let out = upsert_block("export FOO=1", Shell::Bash, &bin());
        assert!(out.starts_with("export FOO=1\n\n# >>> yerd PATH >>>"));
        assert!(out.matches(MARKER_OPEN).count() == 1);
    }

    #[test]
    fn upsert_replaces_in_place_preserving_surroundings() {
        let original = format!("A\n\n{}B\nC\n", render_block(Shell::Zsh, &bin()));
        let out = upsert_block(&original, Shell::Zsh, &bin());
        assert!(out.starts_with("A\n"));
        assert!(out.contains("B\nC\n"));
        assert_eq!(out.matches(MARKER_OPEN).count(), 1);
    }

    #[test]
    fn remove_is_exact_inverse_of_append() {
        let before = "# my zshrc\nexport FOO=1\n";
        let with = upsert_block(before, Shell::Zsh, &bin());
        let without = remove_block(&with);
        assert_eq!(without, before);
        assert!(!contains_block(&without));
    }

    #[test]
    fn remove_does_not_eat_a_nonblank_line_above_the_marker() {
        let block = render_block(Shell::Zsh, &bin());
        let text = format!("export KEEP=1\n{block}");
        let out = remove_block(&text);
        assert_eq!(out, "export KEEP=1\n");
    }

    #[test]
    fn remove_on_block_only_file_yields_empty() {
        let only = render_block(Shell::Fish, &bin());
        assert_eq!(remove_block(&only), "");
    }

    #[test]
    fn remove_without_block_is_noop() {
        let s = "nothing to see\n";
        assert_eq!(remove_block(s), s);
    }

    #[test]
    fn a_user_line_mentioning_yerd_is_not_a_marker() {
        let s = "# install yerd PATH stuff manually\nexport FOO=1\n";
        assert!(!contains_block(s));
        assert_eq!(remove_block(s), s);
    }
}
