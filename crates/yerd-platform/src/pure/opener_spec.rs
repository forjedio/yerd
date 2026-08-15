//! Pure command priority for opening paths with the desktop environment.

const KDE_OPENERS: &[&str] = &["kde-open", "kde-open5", "gio", "xdg-open", "gnome-open"];
const GENERIC_OPENERS: &[&str] = &["gio", "xdg-open", "gnome-open", "kde-open", "kde-open5"];

/// Return the Linux default-opener order for the current desktop session.
#[must_use]
pub const fn linux_default_openers(kde_session: bool) -> &'static [&'static str] {
    if kde_session {
        KDE_OPENERS
    } else {
        GENERIC_OPENERS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kde_prefers_native_openers() {
        assert_eq!(
            linux_default_openers(true).first().copied(),
            Some("kde-open")
        );
    }

    #[test]
    fn generic_desktops_prefer_gio_then_xdg() {
        assert_eq!(
            linux_default_openers(false),
            &["gio", "xdg-open", "gnome-open", "kde-open", "kde-open5"]
        );
    }
}
