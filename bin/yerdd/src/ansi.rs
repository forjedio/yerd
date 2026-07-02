//! Strip ANSI escape sequences from subprocess output.
//!
//! Scaffolding/install jobs force `NO_COLOR=1` + `TERM=dumb` and `--no-ansi` on
//! the child (see [`crate::create_site`]), but some tools (npm, git, and
//! Laravel's own installer spinner among them) still emit colour or
//! cursor-control escapes regardless of those hints. The job log is rendered in
//! a plain `<pre>` with no terminal emulator, so anything that slips through
//! renders as literal garbage. [`strip`] is a last-resort filter applied to
//! every line before it's stored, per [`crate::jobs::JobRegistry::push_log`].

/// Remove ANSI escape sequences (CSI, OSC, and two-byte `ESC x` forms) from
/// `input`, passing all other characters through unchanged.
///
/// Works over `char`s (via `.get()`, never indexing) so multi-byte UTF-8 text
/// around an escape sequence is never split mid-codepoint.
///
/// Recognises three escape forms, each consumed in full:
/// - CSI: `ESC '[' [0x30-0x3F]* [0x20-0x2F]* [0x40-0x7E]` (colour, cursor
///   movement, private-mode toggles like `?25l`/`?25h`, etc).
/// - OSC: `ESC ']' ...` terminated by BEL or `ESC '\'` (e.g. window-title
///   sequences).
/// - Two-byte forms, e.g. `ESC c`, `ESC =`, `ESC 7`.
pub fn strip(input: &str) -> String {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if c != '\u{1b}' {
            out.push(c);
            i += 1;
            continue;
        }
        match chars.get(i + 1) {
            Some('[') => {
                let mut j = i + 2;
                while matches!(chars.get(j), Some(c) if ('0'..='?').contains(c)) {
                    j += 1;
                }
                while matches!(chars.get(j), Some(c) if (' '..='/').contains(c)) {
                    j += 1;
                }
                if matches!(chars.get(j), Some(c) if ('@'..='~').contains(c)) {
                    j += 1;
                }
                i = j;
            }
            Some(']') => {
                let mut j = i + 2;
                loop {
                    match chars.get(j) {
                        None => break,
                        Some('\u{7}') => {
                            j += 1;
                            break;
                        }
                        Some('\u{1b}') if chars.get(j + 1) == Some(&'\\') => {
                            j += 2;
                            break;
                        }
                        Some(_) => j += 1,
                    }
                }
                i = j;
            }
            Some(_) => i += 2,
            None => i += 1,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passes_plain_text_through_unchanged() {
        assert_eq!(
            strip("Creating Laravel application..."),
            "Creating Laravel application..."
        );
    }

    #[test]
    fn strips_sgr_color_codes() {
        assert_eq!(strip("\x1b[36mCreating\x1b[39m"), "Creating");
    }

    #[test]
    fn strips_cursor_hide_show() {
        assert_eq!(strip("\x1b[?25lworking\x1b[?25h"), "working");
    }

    #[test]
    fn strips_cursor_movement_and_column_reset() {
        assert_eq!(strip("\x1b[2A\x1b[999Ddone"), "done");
    }

    #[test]
    fn strips_osc_terminated_by_bel() {
        assert_eq!(strip("\x1b]0;title\x07visible"), "visible");
    }

    #[test]
    fn strips_osc_terminated_by_string_terminator() {
        assert_eq!(strip("\x1b]0;title\x1b\\visible"), "visible");
    }

    #[test]
    fn strips_two_byte_escape() {
        assert_eq!(strip("\x1b=visible"), "visible");
    }

    #[test]
    fn preserves_multi_byte_utf8_around_escapes() {
        assert_eq!(
            strip("\x1b[32m✔\x1b[39m Creating Laravel application…"),
            "✔ Creating Laravel application…"
        );
    }

    #[test]
    fn dangling_escape_at_end_of_input_is_dropped() {
        assert_eq!(strip("done\x1b"), "done");
    }
}
