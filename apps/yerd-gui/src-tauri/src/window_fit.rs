//! Pure geometry for fitting a window's configured launch size onto a display.
//!
//! All sizes are logical points. The fitted size keeps a small margin inside
//! the work area so a fitted window never matches the work area exactly: on
//! macOS a frame equal to the visible frame reads as zoomed (see `mac_zoom`),
//! which would make the first zoom click "restore" to an oversized frame.

/// Gap kept between a fitted window and each edge of the work area.
pub(crate) const FIT_MARGIN: f64 = 12.0;

/// The configured size, shrunk where needed to fit inside `area` less
/// [`FIT_MARGIN`] on every side. Never returns a negative dimension.
pub(crate) fn fit_size(configured: (f64, f64), area: (f64, f64)) -> (f64, f64) {
    let max_w = (area.0 - 2.0 * FIT_MARGIN).max(0.0);
    let max_h = (area.1 - 2.0 * FIT_MARGIN).max(0.0);
    (configured.0.min(max_w), configured.1.min(max_h))
}

/// Whether the configured size needs shrinking to fit `area` with the margin.
pub(crate) fn needs_fit(configured: (f64, f64), area: (f64, f64)) -> bool {
    fit_size(configured, area) != configured
}

/// The origin that centres a window of `size` within an area at `origin` of
/// `area` size.
pub(crate) fn centered_origin(
    size: (f64, f64),
    origin: (f64, f64),
    area: (f64, f64),
) -> (f64, f64) {
    (
        origin.0 + (area.0 - size.0) / 2.0,
        origin.1 + (area.1 - size.1) / 2.0,
    )
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::{centered_origin, fit_size, needs_fit, FIT_MARGIN};

    #[test]
    fn fit_size_shrinks_only_the_dimensions_that_overflow() {
        let configured = (1312.0, 860.0);
        for (area, expected) in [
            ((2560.0, 1415.0), (1312.0, 860.0)),
            ((1440.0, 875.0), (1312.0, 851.0)),
            ((1280.0, 775.0), (1256.0, 751.0)),
            ((1366.0, 740.0), (1312.0, 716.0)),
            ((1312.0, 860.0), (1288.0, 836.0)),
            ((10.0, 10.0), (0.0, 0.0)),
        ] {
            assert_eq!(fit_size(configured, area), expected, "area {area:?}");
        }
    }

    #[test]
    fn needs_fit_is_false_only_when_the_margin_still_fits() {
        let configured = (1312.0, 860.0);
        let exact = (1312.0 + 2.0 * FIT_MARGIN, 860.0 + 2.0 * FIT_MARGIN);
        for (area, expected) in [
            ((2560.0, 1415.0), false),
            (exact, false),
            ((exact.0 - 1.0, exact.1), true),
            ((1440.0, 875.0), true),
        ] {
            assert_eq!(needs_fit(configured, area), expected, "area {area:?}");
        }
    }

    #[test]
    fn a_fitted_size_never_reads_as_filling_the_work_area() {
        for area in [(1440.0, 875.0), (1280.0, 775.0), (1024.0, 700.0)] {
            let (w, h) = fit_size((1312.0, 860.0), area);
            let fills = (area.0 - w).abs() < 1.0 && (area.1 - h).abs() < 1.0;
            assert!(!fills, "area {area:?}");
        }
    }

    #[test]
    fn centered_origin_splits_the_spare_space_evenly() {
        assert_eq!(
            centered_origin((1000.0, 800.0), (100.0, 50.0), (1440.0, 900.0)),
            (320.0, 100.0)
        );
        assert_eq!(
            centered_origin((1256.0, 751.0), (0.0, 25.0), (1280.0, 775.0)),
            (12.0, 37.0)
        );
    }
}
