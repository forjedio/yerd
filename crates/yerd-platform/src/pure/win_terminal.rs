//! Pure command shapes for the Windows terminal-launch fallback chain.
//!
//! A directory goes in, the `(program, argv, needs-new-console)` for each
//! terminal we try comes out. The `os::windows` impl owns the actual spawning
//! and first-success-wins loop; keeping the shapes here makes the probe order
//! and per-terminal flags table-testable without spawning anything.

use std::ffi::OsString;
use std::path::Path;

/// A Windows terminal Yerd knows how to open at a working directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WinTerminal {
    /// Windows Terminal (`wt.exe`, an app-execution alias resolved on PATH).
    WindowsTerminal,
    /// Windows PowerShell (`powershell.exe`).
    PowerShell,
    /// Legacy console (`cmd.exe`).
    Cmd,
}

/// Probe order: Windows Terminal first (the modern default), then PowerShell,
/// then `cmd.exe` as the always-present last resort.
pub const WIN_TERMINAL_PROBES: [WinTerminal; 3] = [
    WinTerminal::WindowsTerminal,
    WinTerminal::PowerShell,
    WinTerminal::Cmd,
];

impl WinTerminal {
    /// The executable name to spawn. `wt.exe` is an app-execution alias, so a
    /// PATH lookup is correct (terminal launch is not a security surface, the
    /// Linux impl PATH-probes its terminals the same way).
    #[must_use]
    pub const fn program(self) -> &'static str {
        match self {
            WinTerminal::WindowsTerminal => "wt.exe",
            WinTerminal::PowerShell => "powershell.exe",
            WinTerminal::Cmd => "cmd.exe",
        }
    }

    /// Whether the process must be spawned with `CREATE_NEW_CONSOLE`. `wt.exe`
    /// owns its own window; the two shells would otherwise inherit (and share)
    /// the daemon/GUI's console rather than opening a visible terminal.
    #[must_use]
    pub const fn needs_new_console(self) -> bool {
        !matches!(self, WinTerminal::WindowsTerminal)
    }

    /// The argv (after the program) that opens this terminal at `dir`. `wt.exe`
    /// takes `-d <dir>`; the two shells take a keep-open flag and rely on the
    /// caller setting the process working directory.
    #[must_use]
    pub fn args(self, dir: &Path) -> Vec<OsString> {
        match self {
            WinTerminal::WindowsTerminal => vec![OsString::from("-d"), dir.as_os_str().to_owned()],
            WinTerminal::PowerShell => vec![OsString::from("-NoExit")],
            WinTerminal::Cmd => vec![OsString::from("/K")],
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn probe_order_is_wt_then_powershell_then_cmd() {
        assert_eq!(
            WIN_TERMINAL_PROBES,
            [
                WinTerminal::WindowsTerminal,
                WinTerminal::PowerShell,
                WinTerminal::Cmd
            ]
        );
    }

    #[test]
    fn only_wt_carries_the_directory_in_argv() {
        let dir = PathBuf::from(r"C:\sites\blog");
        assert_eq!(
            WinTerminal::WindowsTerminal.args(&dir),
            vec![OsString::from("-d"), OsString::from(r"C:\sites\blog")]
        );
        assert_eq!(
            WinTerminal::PowerShell.args(&dir),
            vec![OsString::from("-NoExit")]
        );
        assert_eq!(WinTerminal::Cmd.args(&dir), vec![OsString::from("/K")]);
    }

    #[test]
    fn shells_need_a_new_console_but_wt_does_not() {
        assert!(!WinTerminal::WindowsTerminal.needs_new_console());
        assert!(WinTerminal::PowerShell.needs_new_console());
        assert!(WinTerminal::Cmd.needs_new_console());
    }

    #[test]
    fn program_names_are_the_expected_executables() {
        assert_eq!(WinTerminal::WindowsTerminal.program(), "wt.exe");
        assert_eq!(WinTerminal::PowerShell.program(), "powershell.exe");
        assert_eq!(WinTerminal::Cmd.program(), "cmd.exe");
    }
}
