//! Host IDE detection and project launching abstraction.

use std::path::Path;

use crate::PlatformError;

/// Supported IDE identifiers exposed to the GUI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ide {
    /// Microsoft Visual Studio Code.
    VsCode,
    /// Cursor.
    Cursor,
    /// Zed.
    Zed,
    /// Sublime Text.
    Sublime,
    /// `JetBrains` `PhpStorm`.
    PhpStorm,
    /// Windsurf.
    Windsurf,
}

impl Ide {
    /// Return every IDE known to the current protocol.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::VsCode,
            Self::Cursor,
            Self::Zed,
            Self::Sublime,
            Self::PhpStorm,
            Self::Windsurf,
        ]
    }

    /// Return the stable identifier used by GUI actions and IPC arguments.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::VsCode => "vscode",
            Self::Cursor => "cursor",
            Self::Zed => "zed",
            Self::Sublime => "sublime",
            Self::PhpStorm => "phpstorm",
            Self::Windsurf => "windsurf",
        }
    }

    /// Return the user-facing name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::VsCode => "VS Code",
            Self::Cursor => "Cursor",
            Self::Zed => "Zed",
            Self::Sublime => "Sublime Text",
            Self::PhpStorm => "PhpStorm",
            Self::Windsurf => "Windsurf",
        }
    }

    /// Parse a stable GUI action identifier.
    #[must_use]
    pub fn from_wire(value: &str) -> Option<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|ide| ide.wire_name() == value)
    }
}

impl std::fmt::Display for Ide {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.display_name())
    }
}

/// Detect installed IDEs and open project directories in a selected IDE.
pub trait IdeLauncher {
    /// Return supported IDEs available on this host.
    fn installed_ides(&self) -> Vec<Ide>;

    /// Open `path` in `ide`.
    fn open_in_ide(&self, ide: Ide, path: &Path) -> Result<(), PlatformError>;
}
