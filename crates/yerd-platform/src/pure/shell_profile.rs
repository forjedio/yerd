//! Compose and edit the yerd-owned PATH block in a shell rc file.
//!
//! Every function here is pure: string→string, no I/O, no env, no clock. The
//! binary edge (`yerd path`) reads `$SHELL`/`$HOME`, picks the rc file(s), reads
//! their contents, calls [`upsert_block`]/[`remove_block`], and writes back.
//!
//! The block adds `{data}/bin` (where yerd keeps its `php`/`composer` shims) to
//! `PATH`, **prepended** so it wins over other managers (e.g. Herd) and
//! **guarded** so re-sourcing an rc file never appends a duplicate entry. A
//! headless install also passes its install dir, so `yerd` itself lands on
//! `PATH` through the same block.

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

/// Bash's login files, in the order a login shell looks for them. It reads
/// only the first that exists.
pub const BASH_LOGIN_FILES: [&str; 3] = [".bash_profile", ".bash_login", ".profile"];

/// The Bash login file to add the block to, given which of
/// [`BASH_LOGIN_FILES`] exist: the first existing one (the only one Bash
/// reads), else `.bash_profile`. Creating `.bash_profile` beside an existing
/// `.profile` would make Bash stop reading `.profile`, dropping whatever it
/// sets up (on Debian and Ubuntu, sourcing `.bashrc` and adding `~/bin`).
#[must_use]
pub fn bash_login_relpath(exists: impl Fn(&str) -> bool) -> PathBuf {
    let name = BASH_LOGIN_FILES
        .iter()
        .copied()
        .find(|name| exists(name))
        .unwrap_or(".bash_profile");
    PathBuf::from(name)
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

/// The shell body (no markers) that prepends each of `bin_dirs` to `PATH`, in
/// order (each guarded so a repeated `source` never stacks a duplicate entry),
/// and exports `PHPRC` to Yerd's generated CLI ini (`{data}/php-cli.ini`, a
/// sibling of the first dir, `{data}/bin`) so the `php` shim on this PATH picks
/// up Yerd's opinionated CLI defaults (memory limit etc.). Values are
/// double-quoted *and* metacharacter-escaped, so a dir containing a space, `$`,
/// `` ` ``, `\`, or `"` is emitted safely. An absent ini file is harmless - PHP
/// ignores it.
#[must_use]
pub fn render_body(shell: Shell, bin_dirs: &[&Path]) -> String {
    let raw_phprc = bin_dirs
        .first()
        .and_then(|d| d.parent())
        .map(|d| d.join("php-cli.ini").display().to_string());
    let mut lines: Vec<String> = bin_dirs
        .iter()
        .map(|d| path_prepend(shell, &d.display().to_string()))
        .collect();
    if let Some(ini) = raw_phprc {
        lines.push(match shell {
            Shell::Fish => format!("set -gx PHPRC \"{}\"", esc_fish_dq(&ini)),
            Shell::Zsh | Shell::Bash | Shell::Posix => {
                format!("export PHPRC=\"{}\"", esc_posix_dq(&ini))
            }
        });
    }
    lines.join("\n")
}

/// One guarded PATH prepend of `raw_dir` in `shell`'s syntax.
fn path_prepend(shell: Shell, raw_dir: &str) -> String {
    match shell {
        Shell::Fish => {
            let dir = esc_fish_dq(raw_dir);
            format!("if not contains \"{dir}\" $PATH\n    set -gx PATH \"{dir}\" $PATH\nend")
        }
        Shell::Zsh | Shell::Bash | Shell::Posix => {
            let dir = esc_posix_dq(raw_dir);
            format!(
                "case \":$PATH:\" in\n  *\":{dir}:\"*) ;;\n  *) export PATH=\"{dir}:$PATH\" ;;\nesac"
            )
        }
    }
}

/// The full managed block (markers + body), with a trailing newline.
#[must_use]
pub fn render_block(shell: Shell, bin_dirs: &[&Path]) -> String {
    format!(
        "{MARKER_OPEN}\n{}\n{MARKER_CLOSE}\n",
        render_body(shell, bin_dirs)
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
pub fn upsert_block(existing: &str, shell: Shell, bin_dirs: &[&Path]) -> String {
    let block = render_block(shell, bin_dirs);
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

/// Whether Yerd's `bin_dir` still needs wiring onto the user's `PATH`.
///
/// `login_path` is the `PATH` the user's login shell builds (when it could be
/// probed); `rc_contents` are the user's rc files. Either source showing the
/// directory is enough: the probe catches PATH wired by a dotfiles manager or a
/// sourced file, while the rc scan covers what a login shell can miss (a bash
/// login shell never reads `.bashrc` unless `.bash_profile` sources it) and a
/// `yerd path install` made since the probe's PATH was cached.
#[must_use]
pub fn bin_dir_needs_setup(
    login_path: Option<&[PathBuf]>,
    bin_dir: &Path,
    home: &Path,
    rc_contents: &[String],
) -> bool {
    let on_login_path = login_path.is_some_and(|dirs| dirs.iter().any(|d| d == bin_dir));
    let in_rc = rc_contents
        .iter()
        .any(|c| contains_block(c) || mentions_dir(c, bin_dir, home));
    !(on_login_path || in_rc)
}

/// True if a line of `content` that sets `PATH` spells out `dir` as a whole
/// path entry, either absolutely or as `$HOME/..`, `${HOME}/..`, or `~/..`
/// relative to `home`. Comments are ignored. A heuristic: lines naming a file
/// inside `dir` (an alias to `{dir}/composer`), a sibling (`{dir}-old`), a
/// longer path ending in `dir`, or another variable (`MANPATH`) don't count.
#[must_use]
pub fn mentions_dir(content: &str, dir: &Path, home: &Path) -> bool {
    let mut spellings = vec![dir.display().to_string()];
    if let Ok(rel) = dir.strip_prefix(home) {
        let rel = rel.display();
        spellings.push(format!("$HOME/{rel}"));
        spellings.push(format!("${{HOME}}/{rel}"));
        spellings.push(format!("~/{rel}"));
    }
    content
        .lines()
        .map(strip_comment)
        .filter(|l| sets_path(l))
        .any(|l| spellings.iter().any(|sp| has_path_entry(l, sp)))
}

/// `line` without its shell comment: everything from the first `#` that
/// starts a word outside quotes.
fn strip_comment(line: &str) -> &str {
    let mut quote = None;
    let mut prev_blank = true;
    for (i, c) in line.char_indices() {
        if let Some(q) = quote {
            if c == q {
                quote = None;
            }
        } else if c == '"' || c == '\'' {
            quote = Some(c);
        } else if c == '#' && prev_blank {
            return line.get(..i).unwrap_or(line);
        }
        prev_blank = c.is_whitespace();
    }
    line
}

/// True if `line` assigns or edits the `PATH` variable: `PATH`/`path` as a
/// whole word that isn't an expansion (`$PATH`, `${PATH}`), or `fish_add_path`.
fn sets_path(line: &str) -> bool {
    if line.contains("fish_add_path") {
        return true;
    }
    let is_word = |c: char| c.is_ascii_alphanumeric() || c == '_';
    ["PATH", "path"].iter().any(|word| {
        line.match_indices(word).any(|(i, _)| {
            let before = line.get(..i).and_then(|b| b.chars().next_back());
            let after = line.get(i + word.len()..).and_then(|a| a.chars().next());
            !before.is_some_and(|c| is_word(c) || c == '$' || c == '{')
                && !after.is_some_and(is_word)
        })
    })
}

/// True if `needle` occurs in `line` as a whole path entry: preceded by the
/// start of the line, `:`, `=`, `(`, a quote, or whitespace, and followed by the
/// end of the line, `:`, a quote, `)`, `;`, or whitespace (a single trailing `/`
/// is allowed).
fn has_path_entry(line: &str, needle: &str) -> bool {
    line.match_indices(needle).any(|(i, _)| {
        let starts = line
            .get(..i)
            .and_then(|b| b.chars().next_back())
            .map_or(true, |c| {
                matches!(c, ':' | '=' | '(' | '"' | '\'') || c.is_whitespace()
            });
        let rest = line.get(i + needle.len()..).unwrap_or("");
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        let ends = rest.chars().next().map_or(true, |c| {
            matches!(c, ':' | '"' | '\'' | ')' | ';') || c.is_whitespace()
        });
        starts && ends
    })
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
    fn bash_login_relpath_picks_the_file_bash_reads() {
        let cases: [(&[&str], &str); 6] = [
            (&[], ".bash_profile"),
            (&[".profile"], ".profile"),
            (&[".bash_login", ".profile"], ".bash_login"),
            (&[".bash_profile", ".profile"], ".bash_profile"),
            (&[".bash_profile", ".bash_login"], ".bash_profile"),
            (&[".bashrc"], ".bash_profile"),
        ];
        for (existing, want) in cases {
            assert_eq!(
                bash_login_relpath(|name| existing.contains(&name)),
                PathBuf::from(want),
                "{existing:?}"
            );
        }
    }

    #[test]
    fn body_is_guarded_and_quotes_the_space() {
        let posix = render_body(Shell::Zsh, &[bin().as_path()]);
        assert!(posix.contains("Application Support"));
        assert!(posix.contains(
            "export PATH=\"/Users/x/Library/Application Support/io.yerd.Yerd/bin:$PATH\""
        ));
        assert!(posix.contains("case \":$PATH:\""));
        assert!(posix.contains(") ;;"));

        assert!(posix.contains(
            "export PHPRC=\"/Users/x/Library/Application Support/io.yerd.Yerd/php-cli.ini\""
        ));

        let fish = render_body(Shell::Fish, &[bin().as_path()]);
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

        let posix = render_body(Shell::Zsh, &[dir.as_path()]);
        assert!(posix.contains(r#"export PATH="/home/b\$x/a\`b/c\\d/e\"f/bin:$PATH""#));
        assert!(posix.contains(r#"export PHPRC="/home/b\$x/a\`b/c\\d/e\"f/php-cli.ini""#));

        let fish = render_body(Shell::Fish, &[dir.as_path()]);
        assert!(fish.contains(r#"set -gx PATH "/home/b\$x/a`b/c\\d/e\"f/bin" $PATH"#));
        assert!(fish.contains(r#"set -gx PHPRC "/home/b\$x/a`b/c\\d/e\"f/php-cli.ini""#));
    }

    fn legacy_one_dir_body(shell: Shell, dir: &str) -> String {
        match shell {
            Shell::Fish => format!(
                "if not contains \"{dir}/bin\" $PATH\n    set -gx PATH \"{dir}/bin\" $PATH\nend\nset -gx PHPRC \"{dir}/php-cli.ini\""
            ),
            Shell::Zsh | Shell::Bash | Shell::Posix => format!(
                "case \":$PATH:\" in\n  *\":{dir}/bin:\"*) ;;\n  *) export PATH=\"{dir}/bin:$PATH\" ;;\nesac\nexport PHPRC=\"{dir}/php-cli.ini\""
            ),
        }
    }

    #[test]
    fn one_dir_body_is_unchanged_for_every_shell() {
        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Posix] {
            assert_eq!(
                render_body(shell, &[Path::new("/home/u/.local/share/yerd/bin")]),
                legacy_one_dir_body(shell, "/home/u/.local/share/yerd"),
                "{shell:?}"
            );
        }
    }

    #[test]
    fn two_dirs_render_both_prepends_in_order_and_one_phprc() {
        let data_bin = Path::new("/home/u/.local/share/yerd/bin");
        let install = Path::new("/home/u/.yerd/bin");
        for shell in [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Posix] {
            let body = render_body(shell, &[data_bin, install]);
            let first = body.find("/home/u/.local/share/yerd/bin").unwrap();
            let second = body.find("/home/u/.yerd/bin").unwrap();
            assert!(first < second, "{shell:?}: {body}");
            assert_eq!(body.matches("PHPRC").count(), 1, "{shell:?}: {body}");
            assert!(
                body.contains("/home/u/.local/share/yerd/php-cli.ini"),
                "{shell:?}: {body}"
            );
            let prepends = if shell == Shell::Fish {
                body.matches("set -gx PATH").count()
            } else {
                body.matches("export PATH=").count()
            };
            assert_eq!(prepends, 2, "{shell:?}: {body}");
        }
    }

    #[test]
    fn upsert_replaces_an_old_one_dir_block_with_two_dirs() {
        let data_bin = Path::new("/home/u/.local/share/yerd/bin");
        let install = Path::new("/home/u/.yerd/bin");
        let old = upsert_block("export FOO=1\n", Shell::Bash, &[data_bin]);
        let new = upsert_block(&old, Shell::Bash, &[data_bin, install]);
        assert_eq!(new.matches(MARKER_OPEN).count(), 1);
        assert!(new.starts_with("export FOO=1\n\n"));
        assert!(new.contains("export PATH=\"/home/u/.yerd/bin:$PATH\""));
        assert_eq!(remove_block(&new), "export FOO=1\n");
    }

    #[test]
    fn upsert_into_empty_file() {
        let out = upsert_block("", Shell::Zsh, &[bin().as_path()]);
        assert!(out.starts_with(MARKER_OPEN));
        assert!(out.trim_end().ends_with(MARKER_CLOSE));
        assert!(contains_block(&out));
    }

    #[test]
    fn upsert_is_idempotent() {
        let once = upsert_block("# my zshrc\nexport FOO=1\n", Shell::Zsh, &[bin().as_path()]);
        let twice = upsert_block(&once, Shell::Zsh, &[bin().as_path()]);
        assert_eq!(once, twice);
        assert_eq!(once.matches(MARKER_OPEN).count(), 1);
    }

    #[test]
    fn upsert_appends_with_single_blank_separator_no_trailing_newline() {
        let out = upsert_block("export FOO=1", Shell::Bash, &[bin().as_path()]);
        assert!(out.starts_with("export FOO=1\n\n# >>> yerd PATH >>>"));
        assert!(out.matches(MARKER_OPEN).count() == 1);
    }

    #[test]
    fn upsert_replaces_in_place_preserving_surroundings() {
        let original = format!(
            "A\n\n{}B\nC\n",
            render_block(Shell::Zsh, &[bin().as_path()])
        );
        let out = upsert_block(&original, Shell::Zsh, &[bin().as_path()]);
        assert!(out.starts_with("A\n"));
        assert!(out.contains("B\nC\n"));
        assert_eq!(out.matches(MARKER_OPEN).count(), 1);
    }

    #[test]
    fn remove_is_exact_inverse_of_append() {
        let before = "# my zshrc\nexport FOO=1\n";
        let with = upsert_block(before, Shell::Zsh, &[bin().as_path()]);
        let without = remove_block(&with);
        assert_eq!(without, before);
        assert!(!contains_block(&without));
    }

    #[test]
    fn remove_does_not_eat_a_nonblank_line_above_the_marker() {
        let block = render_block(Shell::Zsh, &[bin().as_path()]);
        let text = format!("export KEEP=1\n{block}");
        let out = remove_block(&text);
        assert_eq!(out, "export KEEP=1\n");
    }

    #[test]
    fn remove_on_block_only_file_yields_empty() {
        let only = render_block(Shell::Fish, &[bin().as_path()]);
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

    fn home() -> PathBuf {
        PathBuf::from("/Users/x")
    }

    type SetupCase<'a> = (Option<&'a [PathBuf]>, &'a [String], bool);

    #[test]
    fn bin_dir_needs_setup_table() {
        let d = bin();
        let on_path = vec![PathBuf::from("/usr/bin"), d.clone()];
        let trailing = vec![PathBuf::from(format!("{}/", d.display()))];
        let off_path = vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")];
        let block = vec![upsert_block("", Shell::Zsh, &[d.as_path()])];
        let literal = vec![format!("export PATH=\"{}:$PATH\"", d.display())];
        let unrelated = vec!["export PATH=\"/usr/bin\"".to_owned()];
        let cases: [SetupCase<'_>; 9] = [
            (Some(&on_path), &[], false),
            (Some(&trailing), &[], false),
            (Some(&off_path), &[], true),
            (Some(&off_path), &block, false),
            (Some(&off_path), &literal, false),
            (None, &block, false),
            (None, &literal, false),
            (None, &unrelated, true),
            (None, &[], true),
        ];
        for (i, (login, rc, want)) in cases.into_iter().enumerate() {
            assert_eq!(
                bin_dir_needs_setup(login, &d, &home(), rc),
                want,
                "case {i}"
            );
        }
    }

    #[test]
    fn mentions_dir_table() {
        let d = bin();
        let ds = d.display().to_string();
        let rel = "Library/Application Support/io.yerd.Yerd/bin";
        let hits = [
            format!("export PATH=\"{ds}:$PATH\""),
            format!("export PATH={ds}"),
            format!("  PATH=\"$HOME/{rel}/:$PATH\""),
            format!("PATH=\"${{HOME}}/{rel}:$PATH\""),
            format!("path+=(~/{rel})"),
            format!("fish_add_path '{ds}'"),
            format!("set -gx PATH \"{ds}\" $PATH # yerd"),
            format!("export PATH=\"/opt/a#b:{ds}:$PATH\""),
        ];
        for line in &hits {
            assert!(mentions_dir(line, &d, &home()), "{line}");
        }
        let misses = [
            format!("# export PATH=\"{ds}:$PATH\""),
            format!("alias composer=\"{ds}/composer\""),
            format!("export PATH=\"{ds}-old:$PATH\""),
            format!("export PATH=\"{ds}/sub:$PATH\""),
            format!("cd {ds}"),
            format!("export FOO=1 # PATH {ds}"),
            format!("export PATH=\"/opt{ds}:$PATH\""),
            format!("export MANPATH=\"{ds}:$MANPATH\""),
            format!("alias p=\"echo $PATH {ds}\""),
            "export PATH=\"/usr/bin\"".to_owned(),
            String::new(),
        ];
        for line in &misses {
            assert!(!mentions_dir(line, &d, &home()), "{line}");
        }
    }
}
