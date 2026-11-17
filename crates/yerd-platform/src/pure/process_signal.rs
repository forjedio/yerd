//! Pure guards and decoders for [`crate::ProcessTable`] implementations.

/// The `i32` PID to hand `killpg`/`kill`, or `None` when it must never be
/// signalled: pid 0 (`killpg(0, ..)` targets the caller's own group), pid 1
/// (init, or launchd on macOS), or a value that overflows `i32` (it would wrap
/// to a negative pid, which `kill` treats as a process group).
#[must_use]
pub fn signal_target(pid: u32) -> Option<i32> {
    if pid <= 1 {
        return None;
    }
    i32::try_from(pid).ok()
}

/// Decode Linux `/proc/<pid>/cmdline` bytes (NUL-separated arguments, usually
/// NUL-terminated) into one space-joined string. `None` when empty, as for a
/// kernel thread or a zombie.
#[must_use]
pub fn decode_proc_cmdline(raw: &[u8]) -> Option<String> {
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8_lossy(arg).into_owned())
        .collect();
    (!args.is_empty()).then(|| args.join(" "))
}

/// Trim `ps -o command=` output to the command line, `None` when empty (the
/// process is gone).
#[must_use]
pub fn decode_ps_command(stdout: &str) -> Option<String> {
    let line = stdout.trim();
    (!line.is_empty()).then(|| line.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_target_table() {
        let cases: &[(u32, Option<i32>)] = &[
            (0, None),
            (1, None),
            (2, Some(2)),
            (4242, Some(4242)),
            (2_147_483_647, Some(i32::MAX)),
            (2_147_483_648, None),
            (u32::MAX, None),
        ];
        for (pid, expected) in cases {
            assert_eq!(signal_target(*pid), *expected, "pid {pid}");
        }
    }

    #[test]
    fn decode_proc_cmdline_table() {
        let cases: &[(&[u8], Option<&str>)] = &[
            (b"", None),
            (b"\0", None),
            (
                b"/usr/bin/cloudflared\0tunnel\0",
                Some("/usr/bin/cloudflared tunnel"),
            ),
            (
                b"/a b/cloudflared\0--config\0/data/tunnel/quick.yml",
                Some("/a b/cloudflared --config /data/tunnel/quick.yml"),
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(decode_proc_cmdline(raw).as_deref(), *expected, "{raw:?}");
        }
    }

    #[test]
    fn decode_ps_command_table() {
        assert_eq!(decode_ps_command(""), None);
        assert_eq!(decode_ps_command("  \n"), None);
        assert_eq!(
            decode_ps_command("/bin/cloudflared tunnel run\n").as_deref(),
            Some("/bin/cloudflared tunnel run")
        );
    }
}
